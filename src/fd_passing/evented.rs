use crate::http::{self, BUF_SIZE, BufferStep, Request};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONTROL_TOKEN: u64 = u64::MAX - 1;
const LISTENER_TOKEN: u64 = u64::MAX;
const MAX_EVENTS: i32 = 256;
const RECV_FD_BUDGET_DEFAULT: i32 = 32;
const MAX_CLIENT_FD: usize = 65_536;

#[repr(C)]
struct EpollParams {
    busy_poll_usecs: u32,
    busy_poll_budget: u16,
    prefer_busy_poll: u8,
    _pad: u8,
}

const EPIOCSPARAMS: libc::c_ulong = 0x40087001;

struct ConnTable {
    slots: Vec<Option<ConnState>>,
    /// Last `EPOLLIN` / `EPOLLOUT` registered for each fd (`None` = not in epoll).
    epoll_interest: Vec<Option<i32>>,
    buf_pool: Vec<Box<[u8; BUF_SIZE]>>,
    buf_pool_cap: usize,
}

impl ConnTable {
    fn new() -> Self {
        let buf_pool_cap = std::env::var("RINHA_BUF_POOL_INIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);

        // Pre-allocate the full FD index to avoid Vec resize/realloc during the benchmark.
        // This reduces tail outliers caused by occasional large FD values.
        let mut slots: Vec<Option<ConnState>> = Vec::with_capacity(MAX_CLIENT_FD);
        slots.resize_with(MAX_CLIENT_FD, || None);

        let mut epoll_interest: Vec<Option<i32>> = Vec::with_capacity(MAX_CLIENT_FD);
        epoll_interest.resize_with(MAX_CLIENT_FD, || None);

        let mut buf_pool = Vec::with_capacity(buf_pool_cap);
        for _ in 0..buf_pool_cap {
            buf_pool.push(Box::new([0u8; BUF_SIZE]));
        }

        Self {
            slots,
            epoll_interest,
            buf_pool,
            buf_pool_cap,
        }
    }

    #[inline(always)]
    fn epoll_interest(&self, fd: RawFd) -> Option<i32> {
        self.epoll_interest.get(fd as usize).copied().flatten()
    }

    #[inline(always)]
    fn set_epoll_interest(&mut self, fd: RawFd, interest: Option<i32>) {
        let idx = fd as usize;
        if idx < self.epoll_interest.len() {
            self.epoll_interest[idx] = interest;
        }
    }

    #[inline(always)]
    fn alloc_buf(&mut self) -> Box<[u8; BUF_SIZE]> {
        self.buf_pool
            .pop()
            .unwrap_or_else(|| Box::new([0u8; BUF_SIZE]))
    }

    #[inline(always)]
    fn recycle_buf(&mut self, buf: Box<[u8; BUF_SIZE]>) {
        if self.buf_pool_cap > 0 && self.buf_pool.len() < self.buf_pool_cap {
            self.buf_pool.push(buf);
        }
        // else: drop
    }

    #[inline(always)]
    fn recycle_conn_state_buf(&mut self, state: ConnState) {
        match state {
            ConnState::Reading { buf, .. } => self.recycle_buf(buf),
            ConnState::Writing { buf, .. } => self.recycle_buf(buf),
        }
    }

    fn insert(&mut self, fd: RawFd, state: ConnState) {
        let idx = fd as usize;
        if idx >= MAX_CLIENT_FD {
            return;
        }
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(state);
    }

    fn remove(&mut self, fd: RawFd) -> Option<ConnState> {
        self.slots.get_mut(fd as usize)?.take()
    }
}

type Handler = dyn Fn(&Request) -> &'static [u8] + Send + Sync;

#[derive(Clone)]
enum ConnState {
    Reading {
        buf: Box<[u8; BUF_SIZE]>,
        used: usize,
    },
    Writing {
        buf: Box<[u8; BUF_SIZE]>,
        response: &'static [u8],
        written: usize,
        leftover_off: usize,
        leftover_len: usize,
        keep_alive: bool,
    },
}

pub fn run_fd_evented_server<F>(socket_path: &str, handler: F)
where
    F: Fn(&Request) -> &'static [u8] + Send + Sync + 'static,
{
    ignore_sigpipe();

    let handler: Arc<Handler> = Arc::new(handler);
    let _ = std::fs::remove_file(socket_path);

    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind fd socket {}: {}", socket_path, e);
            return;
        }
    };

    set_nonblocking(listener.as_raw_fd()).ok();

    let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epoll_fd < 0 {
        eprintln!("epoll_create1 failed: {}", io::Error::last_os_error());
        return;
    }

    configure_epoll_busy_poll(epoll_fd);
    let epoll_timeout_ms = epoll_timeout_ms_from_env();
    let spin_before_block_us = spin_before_block_us_from_env();
    let recv_fd_budget = recv_fd_budget_from_env();
    let accept_budget = accept_budget_from_env();
    let client_fd_preconfigured = client_fd_preconfigured_from_env();

    epoll_add(
        epoll_fd,
        listener.as_raw_fd(),
        LISTENER_TOKEN,
        libc::EPOLLIN,
    );

    let mut control: Option<UnixStream> = None;
    let mut conns = ConnTable::new();
    let mut events = [libc::epoll_event { events: 0, u64: 0 }; MAX_EVENTS as usize];

    loop {
        let ready = if spin_before_block_us > 0 {
            epoll_wait_spin_then_block(
                epoll_fd,
                &mut events,
                spin_before_block_us,
                epoll_timeout_ms,
            )
        } else {
            unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, epoll_timeout_ms) }
        };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            eprintln!("epoll_wait error: {err}");
            break;
        }
        if ready == 0 {
            continue;
        }

        for i in 0..ready as usize {
            let token = events[i].u64;
            let revents = events[i].events;

            if token == LISTENER_TOKEN {
                accept_control(&listener, epoll_fd, accept_budget, &mut control);
                continue;
            }

            if token == CONTROL_TOKEN {
                if revents & (libc::EPOLLIN as u32) != 0 {
                    if let Some(stream) = control.as_mut() {
                        drain_fds(
                            stream,
                            epoll_fd,
                            recv_fd_budget,
                            &handler,
                            &mut conns,
                            client_fd_preconfigured,
                        );
                    }
                }
                if revents & ((libc::EPOLLHUP | libc::EPOLLERR) as u32) != 0 {
                    if let Some(fd) = control.as_ref().map(AsRawFd::as_raw_fd) {
                        epoll_del(epoll_fd, fd);
                    }
                    control = None;
                }
                continue;
            }

            let client_fd = token as RawFd;
            if revents & ((libc::EPOLLHUP | libc::EPOLLERR | libc::EPOLLRDHUP) as u32) != 0 {
                close_conn(epoll_fd, client_fd, &mut conns);
                continue;
            }

            if revents & (libc::EPOLLIN as u32) != 0 {
                on_readable(epoll_fd, client_fd, &handler, &mut conns);
            }
            if revents & (libc::EPOLLOUT as u32) != 0 {
                on_writable(epoll_fd, client_fd, &handler, &mut conns);
            }
        }
    }
}

fn accept_control(
    listener: &UnixListener,
    epoll_fd: RawFd,
    accept_budget: i32,
    control: &mut Option<UnixStream>,
) {
    // Typically we only need a single LB->API control connection. Still, bound accepts
    // to avoid pathological bursts.
    let mut accepted = 0i32;
    loop {
        if accept_budget > 0 && accepted >= accept_budget {
            return;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                set_nonblocking(stream.as_raw_fd()).ok();

                if control.is_none() {
                    epoll_add(epoll_fd, stream.as_raw_fd(), CONTROL_TOKEN, libc::EPOLLIN);
                    *control = Some(stream);
                    accepted += 1;
                    // Preserve previous behavior: accept at most one control stream per tick
                    // unless accept_budget is explicitly set > 0.
                    if accept_budget <= 0 {
                        return;
                    }
                } else {
                    // Extra control connections: close immediately (do not replace active one).
                    unsafe { libc::close(stream.as_raw_fd()) };
                    accepted += 1;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) => {
                eprintln!("control accept error: {e}");
                return;
            }
        }
    }
}

fn drain_fds(
    control: &mut UnixStream,
    epoll_fd: RawFd,
    recv_fd_budget: i32,
    handler: &Arc<Handler>,
    conns: &mut ConnTable,
    client_fd_preconfigured: bool,
) {
    for _ in 0..recv_fd_budget {
        match super::recv_fd_nb(control) {
            super::RecvFdResult::Fd(client_fd) => {
                on_new_client_fd(epoll_fd, client_fd, handler, conns, client_fd_preconfigured)
            }
            super::RecvFdResult::WouldBlock => break,
            super::RecvFdResult::Closed => {
                epoll_del(epoll_fd, control.as_raw_fd());
                break;
            }
        }
    }
}

fn on_new_client_fd(
    epoll_fd: RawFd,
    client_fd: RawFd,
    handler: &Arc<Handler>,
    conns: &mut ConnTable,
    client_fd_preconfigured: bool,
) {
    if client_fd as usize >= MAX_CLIENT_FD {
        unsafe { libc::close(client_fd) };
        return;
    }
    if !client_fd_preconfigured {
        if set_nonblocking(client_fd).is_err() {
            unsafe { libc::close(client_fd) };
            return;
        }
        set_tcp_nodelay(client_fd);
    }

    let mut buf = conns.alloc_buf();
    match greedy_read(client_fd, &mut buf, 0) {
        ReadOutcome::Data(used) => {
            drive_reading(epoll_fd, client_fd, handler, conns, buf, used);
        }
        ReadOutcome::WouldBlock => {
            conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLIN);
            conns.insert(client_fd, ConnState::Reading { buf, used: 0 });
        }
        ReadOutcome::Closed => {
            conns.recycle_buf(buf);
            unsafe { libc::close(client_fd) };
        }
    }
}

fn on_readable(epoll_fd: RawFd, client_fd: RawFd, handler: &Arc<Handler>, conns: &mut ConnTable) {
    let (buf, used) = match conns.remove(client_fd) {
        Some(ConnState::Reading { buf, used }) => (buf, used),
        Some(other) => {
            conns.insert(client_fd, other);
            return;
        }
        None => return,
    };

    let mut buf = buf;
    match greedy_read(client_fd, &mut buf, used) {
        ReadOutcome::Data(used) => {
            drive_reading(epoll_fd, client_fd, handler, conns, buf, used);
        }
        ReadOutcome::WouldBlock => {
            conns.insert(client_fd, ConnState::Reading { buf, used });
        }
        ReadOutcome::Closed => {
            conns.recycle_buf(buf);
            close_conn(epoll_fd, client_fd, conns);
        }
    }
}

fn drive_reading(
    epoll_fd: RawFd,
    client_fd: RawFd,
    handler: &Arc<Handler>,
    conns: &mut ConnTable,
    mut buf: Box<[u8; BUF_SIZE]>,
    mut used: usize,
) {
    loop {
        if used >= BUF_SIZE {
            start_write(
                epoll_fd,
                client_fd,
                handler,
                conns,
                buf,
                http::RESPONSE_BAD_REQUEST,
                0,
                0,
                false,
            );
            return;
        }

        let mut processed = 0usize;
        while processed < used {
            match http::process_one_request(&buf[processed..used], |req| handler(req)) {
                BufferStep::Respond {
                    consumed,
                    response,
                    keep_alive,
                } => {
                    processed += consumed;
                    let leftover_off = processed;
                    let leftover_len = used - processed;
                    start_write(
                        epoll_fd,
                        client_fd,
                        handler,
                        conns,
                        buf,
                        response,
                        leftover_off,
                        leftover_len,
                        keep_alive,
                    );
                    return;
                }
                BufferStep::RejectAndClose { response } => {
                    start_write(
                        epoll_fd, client_fd, handler, conns, buf, response, 0, 0, false,
                    );
                    return;
                }
                BufferStep::NeedMore => {
                    if processed > 0 {
                        buf.copy_within(processed..used, 0);
                        used -= processed;
                    }
                    conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLIN);
                    conns.insert(client_fd, ConnState::Reading { buf, used });
                    return;
                }
            }
        }

        if processed > 0 {
            buf.copy_within(processed..used, 0);
            used -= processed;
        }
        conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLIN);
        conns.insert(client_fd, ConnState::Reading { buf, used });
        return;
    }
}

fn start_write(
    epoll_fd: RawFd,
    client_fd: RawFd,
    handler: &Arc<Handler>,
    conns: &mut ConnTable,
    buf: Box<[u8; BUF_SIZE]>,
    response: &'static [u8],
    leftover_off: usize,
    leftover_len: usize,
    keep_alive: bool,
) {
    let state = ConnState::Writing {
        buf,
        response,
        written: 0,
        leftover_off,
        leftover_len,
        keep_alive,
    };
    match finish_write(epoll_fd, client_fd, conns, state) {
        WriteOutcome::DoneReading { buf, used } => {
            if used > 0 {
                drive_reading(epoll_fd, client_fd, handler, conns, buf, used);
            } else if keep_alive {
                conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLIN);
                conns.insert(client_fd, ConnState::Reading { buf, used: 0 });
            } else {
                shutdown_client(epoll_fd, client_fd, conns);
            }
        }
        WriteOutcome::Wait(state) => {
            conns.insert(client_fd, state);
        }
        WriteOutcome::Closed => {}
    }
}

fn on_writable(epoll_fd: RawFd, client_fd: RawFd, handler: &Arc<Handler>, conns: &mut ConnTable) {
    let state = match conns.remove(client_fd) {
        Some(s @ ConnState::Writing { .. }) => s,
        Some(other) => {
            conns.insert(client_fd, other);
            return;
        }
        None => return,
    };

    match finish_write(epoll_fd, client_fd, conns, state) {
        WriteOutcome::DoneReading { buf, used } => {
            if used > 0 {
                drive_reading(epoll_fd, client_fd, handler, conns, buf, used);
            } else {
                conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLIN);
                conns.insert(client_fd, ConnState::Reading { buf, used: 0 });
            }
        }
        WriteOutcome::Wait(state) => {
            conns.insert(client_fd, state);
        }
        WriteOutcome::Closed => {}
    }
}

enum WriteOutcome {
    DoneReading {
        buf: Box<[u8; BUF_SIZE]>,
        used: usize,
    },
    Wait(ConnState),
    Closed,
}

fn finish_write(
    epoll_fd: RawFd,
    client_fd: RawFd,
    conns: &mut ConnTable,
    state: ConnState,
) -> WriteOutcome {
    let ConnState::Writing {
        mut buf,
        response,
        mut written,
        leftover_off,
        leftover_len,
        keep_alive,
    } = state
    else {
        return WriteOutcome::Closed;
    };

    loop {
        let res = unsafe {
            libc::write(
                client_fd,
                response.as_ptr().add(written) as *const libc::c_void,
                response.len() - written,
            )
        };
        if res > 0 {
            written += res as usize;
            if written == response.len() {
                if leftover_len > 0 {
                    buf.copy_within(leftover_off..leftover_off + leftover_len, 0);
                    return WriteOutcome::DoneReading {
                        buf,
                        used: leftover_len,
                    };
                }
                if keep_alive {
                    return WriteOutcome::DoneReading { buf, used: 0 };
                }
                // Connection is closing: recycle buffer to reduce heap churn.
                conns.recycle_buf(buf);
                shutdown_client(epoll_fd, client_fd, conns);
                return WriteOutcome::Closed;
            }
            continue;
        }

        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EAGAIN) {
            conn_arm_epoll(conns, epoll_fd, client_fd, libc::EPOLLOUT);
            return WriteOutcome::Wait(ConnState::Writing {
                buf,
                response,
                written,
                leftover_off,
                leftover_len,
                keep_alive,
            });
        }

        // Non-EAGAIN write error: recycle buffer since the connection is closing.
        conns.recycle_buf(buf);
        shutdown_client(epoll_fd, client_fd, conns);
        return WriteOutcome::Closed;
    }
}

fn shutdown_client(epoll_fd: RawFd, client_fd: RawFd, conns: &mut ConnTable) {
    if conns.epoll_interest(client_fd).is_some() {
        epoll_del(epoll_fd, client_fd);
        conns.set_epoll_interest(client_fd, None);
    }
    unsafe { libc::close(client_fd) };
}

fn close_conn(epoll_fd: RawFd, client_fd: RawFd, conns: &mut ConnTable) {
    if let Some(state) = conns.remove(client_fd) {
        conns.recycle_conn_state_buf(state);
    }
    shutdown_client(epoll_fd, client_fd, conns);
}

fn epoll_timeout_ms_from_env() -> i32 {
    std::env::var("RINHA_EPOLL_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

fn recv_fd_budget_from_env() -> i32 {
    std::env::var("RINHA_RECV_FD_BUDGET")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(RECV_FD_BUDGET_DEFAULT)
}

fn accept_budget_from_env() -> i32 {
    // 0 keeps old behavior (accept at most one control connection per tick).
    std::env::var("RINHA_ACCEPT_BUDGET")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0)
}

fn client_fd_preconfigured_from_env() -> bool {
    std::env::var("RINHA_CLIENT_FD_PRECONFIGURED")
        .map(|v| v != "0")
        .unwrap_or(false)
}

fn configure_epoll_busy_poll(epoll_fd: RawFd) {
    let busy_poll_us: u32 = std::env::var("RINHA_BUSY_POLL_US")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if busy_poll_us == 0 {
        return;
    }
    let budget: u16 = std::env::var("RINHA_BUSY_POLL_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let prefer: u8 = std::env::var("RINHA_PREFER_BUSY_POLL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let params = EpollParams {
        busy_poll_usecs: busy_poll_us,
        busy_poll_budget: budget,
        prefer_busy_poll: prefer,
        _pad: 0,
    };
    let rc = unsafe { libc::ioctl(epoll_fd, EPIOCSPARAMS, &params) };
    if rc < 0 {
        eprintln!(
            "EPIOCSPARAMS failed (busy_poll_us={}): {}",
            busy_poll_us,
            io::Error::last_os_error()
        );
    }
}

enum ReadOutcome {
    Data(usize),
    WouldBlock,
    Closed,
}

fn greedy_read(client_fd: RawFd, buf: &mut [u8; BUF_SIZE], mut used: usize) -> ReadOutcome {
    let had_data = used > 0;
    loop {
        let n = greedy_read_into(client_fd, buf, used);
        if n > 0 {
            used += n as usize;
            if used >= BUF_SIZE {
                return ReadOutcome::Data(used);
            }
            continue;
        }

        if n == 0 {
            return if used > 0 || had_data {
                ReadOutcome::Data(used)
            } else {
                ReadOutcome::Closed
            };
        }

        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(libc::EAGAIN) {
            if used > 0 || had_data {
                ReadOutcome::Data(used)
            } else {
                ReadOutcome::WouldBlock
            }
        } else {
            ReadOutcome::Closed
        };
    }
}

fn greedy_read_into(client_fd: RawFd, buf: &mut [u8; BUF_SIZE], used: usize) -> isize {
    if used >= BUF_SIZE {
        return 0;
    }
    unsafe {
        libc::read(
            client_fd,
            buf.as_mut_ptr().add(used) as *mut libc::c_void,
            BUF_SIZE - used,
        )
    }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_tcp_nodelay(fd: RawFd) {
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_QUICKACK,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

fn epoll_edge_enabled() -> bool {
    std::env::var("RINHA_EPOLL_EDGE")
        .map(|v| v != "0")
        .unwrap_or(true)
}

fn epoll_events(base: i32) -> u32 {
    let mut events = base as u32;
    if epoll_edge_enabled() {
        events |= libc::EPOLLET as u32;
    }
    events
}

fn epoll_add(epoll_fd: RawFd, fd: RawFd, token: u64, events: i32) {
    let mut event = libc::epoll_event {
        events: epoll_events(events),
        u64: token,
    };
    if unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut event) } != 0 {
        eprintln!("epoll_ctl ADD failed: {}", io::Error::last_os_error());
    }
}

fn epoll_mod(epoll_fd: RawFd, fd: RawFd, events: i32) {
    let mut event = libc::epoll_event {
        events: epoll_events(events),
        u64: fd as u64,
    };
    if unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_MOD, fd, &mut event) } != 0 {
        epoll_add(epoll_fd, fd, fd as u64, events);
    }
}

fn spin_before_block_us_from_env() -> u32 {
    std::env::var("RINHA_SPIN_BEFORE_BLOCK_US")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Busy-poll epoll with timeout 0 before blocking, to avoid scheduler wakeup latency.
fn epoll_wait_spin_then_block(
    epoll_fd: RawFd,
    events: &mut [libc::epoll_event],
    spin_us: u32,
    block_timeout_ms: i32,
) -> i32 {
    let deadline = Instant::now() + Duration::from_micros(spin_us as u64);
    loop {
        let ready = unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, 0) };
        if ready != 0 {
            return ready;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::hint::spin_loop();
    }
    unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, block_timeout_ms) }
}

fn conn_arm_epoll(conns: &mut ConnTable, epoll_fd: RawFd, fd: RawFd, events: i32) {
    if conns.epoll_interest(fd) == Some(events) {
        return;
    }
    if conns.epoll_interest(fd).is_some() {
        epoll_mod(epoll_fd, fd, events);
    } else {
        epoll_add(epoll_fd, fd, fd as u64, events);
    }
    conns.set_epoll_interest(fd, Some(events));
}

fn epoll_del(epoll_fd: RawFd, fd: RawFd) {
    let _ = unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
}

fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}
