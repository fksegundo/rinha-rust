use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

const PORT: u16 = 9999;
const UPSTREAMS: [&str; 2] = ["/sockets/api1.sock", "/sockets/api2.sock"];
const BACKLOG: i32 = 65_535;
const MAX_EVENTS: i32 = 256;
const ACCEPT_BATCH_LIMIT: u32 = 64;
const WARMUP_ATTEMPTS: u32 = 300;
const WARMUP_INTERVAL: Duration = Duration::from_millis(10);
const RESPONSE_NOT_READY: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n";
const CONTROL_SNDBUF: i32 = 256 * 1024;
const SOL_TCP: i32 = libc::IPPROTO_TCP;
const TCP_DEFER_ACCEPT: i32 = 9;
const SO_BUSY_POLL: i32 = 46;
const SO_PREFER_BUSY_POLL: i32 = 69;
const SO_BUSY_POLL_BUDGET: i32 = 70;
const WARM_HTTP_HEAD: &[u8] = b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 407\r\n\r\n";
const WARM_HTTP_BODY: &[u8] = b"{\"id\":\"tx-warm\",\"transaction\":{\"amount\":384.88,\"installments\":3,\"requested_at\":\"2026-03-11T20:23:35Z\"},\"customer\":{\"avg_amount\":769.76,\"tx_count_24h\":3,\"known_merchants\":[\"MERC-009\",\"MERC-001\"]},\"merchant\":{\"id\":\"MERC-001\",\"mcc\":\"5912\",\"avg_amount\":298.95},\"terminal\":{\"is_online\":false,\"card_present\":true,\"km_from_home\":13.7},\"last_transaction\":{\"timestamp\":\"2026-03-11T14:58:35Z\",\"km_from_current\":18.8}}";
const _: () = assert!(WARM_HTTP_BODY.len() == 407);

#[derive(Clone)]
struct Config {
    port: u16,
    upstreams: Vec<String>,
    accept_batch_limit: u32,
    skip_accept_tcp_opts: bool,
    tcp_defer_accept: i32,
    busy_poll_us: i32,
    busy_poll_budget: i32,
    prefer_busy_poll: bool,
    self_warm_requests: usize,
}

impl Config {
    fn from_env() -> Self {
        let port = env_u16("LB_PORT")
            .or_else(|| env_u16("PORT"))
            .unwrap_or(PORT);
        let upstreams = std::env::var("LB_BACKENDS")
            .or_else(|_| std::env::var("FD_UPSTREAMS"))
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|paths| !paths.is_empty())
            .unwrap_or_else(|| UPSTREAMS.iter().map(|s| (*s).to_owned()).collect());

        Self {
            port,
            upstreams,
            accept_batch_limit: env_u32("LB_ACCEPT_BATCH").unwrap_or(ACCEPT_BATCH_LIMIT),
            skip_accept_tcp_opts: env_bool("LB_SKIP_ACCEPT_TCP_OPTS", false),
            tcp_defer_accept: env_i32("LB_TCP_DEFER_ACCEPT").unwrap_or(0),
            busy_poll_us: env_i32("LB_BUSY_POLL_US").unwrap_or(0),
            busy_poll_budget: env_i32("LB_BUSY_POLL_BUDGET").unwrap_or(0),
            prefer_busy_poll: env_bool("LB_PREFER_BUSY_POLL", false),
            self_warm_requests: env_usize("LB_SELF_WARM_REQUESTS").unwrap_or(0),
        }
    }
}

struct ControlChannel {
    stream: UnixStream,
    byte: u8,
    control: [u8; 64],
    cmsg_len: usize,
}

impl ControlChannel {
    fn send_fd(&mut self, fd_to_send: RawFd) -> io::Result<()> {
        let mut iov = libc::iovec {
            iov_base: (&mut self.byte as *mut u8).cast(),
            iov_len: 1,
        };
        unsafe {
            let data = self
                .control
                .as_mut_ptr()
                .add(std::mem::size_of::<libc::cmsghdr>())
                .cast::<RawFd>();
            *data = fd_to_send;

            let msg = libc::msghdr {
                msg_name: std::ptr::null_mut(),
                msg_namelen: 0,
                msg_iov: &mut iov,
                msg_iovlen: 1,
                msg_control: self.control.as_mut_ptr().cast(),
                msg_controllen: self.cmsg_len,
                msg_flags: 0,
            };

            let sent = libc::sendmsg(self.stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL);
            if sent != 1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

fn main() {
    ignore_sigpipe();
    let config = Config::from_env();

    let listener = make_listener(&config).expect("failed to bind listener");
    let listener_fd = listener.as_raw_fd();
    set_nonblocking(listener_fd).expect("failed to set listener non-blocking");

    let mut controls = warmup_controls(&config.upstreams);

    let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epoll_fd < 0 {
        panic!("epoll_create1 failed: {}", io::Error::last_os_error());
    }

    let mut event = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: listener_fd as u64,
    };

    if unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, listener_fd, &mut event) } != 0 {
        panic!("epoll_ctl failed: {}", io::Error::last_os_error());
    }

    spawn_self_warm(config.port, config.self_warm_requests);

    let mut upstream_idx = 0usize;
    let mut reconnect_needed = vec![false; config.upstreams.len()];
    let mut events = [libc::epoll_event { events: 0, u64: 0 }; MAX_EVENTS as usize];

    loop {
        reconnect_controls(&config.upstreams, &mut controls, &mut reconnect_needed);

        let ready = unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, -1) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            eprintln!("epoll_wait error: {err}");
            break;
        }

        for i in 0..ready as usize {
            if events[i].u64 as RawFd != listener_fd {
                continue;
            }

            accept_burst(
                listener_fd,
                &mut upstream_idx,
                &mut controls,
                &mut reconnect_needed,
                &config,
            );
        }
    }
}

fn accept_burst(
    listener_fd: RawFd,
    upstream_idx: &mut usize,
    controls: &mut [Option<ControlChannel>],
    reconnect_needed: &mut [bool],
    config: &Config,
) {
    for _ in 0..config.accept_batch_limit {
        let client_fd = unsafe {
            libc::accept4(
                listener_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            )
        };

        if client_fd < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) {
                return;
            }
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            eprintln!("accept error: {err}");
            return;
        }

        if !config.skip_accept_tcp_opts {
            configure_tcp(client_fd);
        }

        if !handoff(client_fd, *upstream_idx, controls, reconnect_needed) {
            let _ = unsafe {
                libc::write(
                    client_fd,
                    RESPONSE_NOT_READY.as_ptr().cast(),
                    RESPONSE_NOT_READY.len(),
                )
            };
        }

        unsafe {
            libc::close(client_fd);
        }

        *upstream_idx += 1;
        if *upstream_idx >= controls.len() {
            *upstream_idx = 0;
        }
    }
}

fn warmup_controls(upstreams: &[String]) -> Vec<Option<ControlChannel>> {
    thread::scope(|scope| {
        upstreams
            .iter()
            .map(|path| {
                scope
                    .spawn(move || warmup_one(path))
                    .join()
                    .expect("warmup thread panicked")
            })
            .collect()
    })
}

fn warmup_one(path: &str) -> Option<ControlChannel> {
    for _ in 0..WARMUP_ATTEMPTS {
        if let Some(stream) = connect_control(path) {
            return Some(stream);
        }
        thread::sleep(WARMUP_INTERVAL);
    }
    None
}

fn reconnect_controls(
    upstreams: &[String],
    controls: &mut [Option<ControlChannel>],
    reconnect_needed: &mut [bool],
) {
    for idx in 0..upstreams.len() {
        if !reconnect_needed[idx] || controls[idx].is_some() {
            continue;
        }

        if let Some(stream) = connect_control(&upstreams[idx]) {
            controls[idx] = Some(stream);
            reconnect_needed[idx] = false;
        }
    }
}

fn handoff(
    client_fd: RawFd,
    first_idx: usize,
    controls: &mut [Option<ControlChannel>],
    reconnect_needed: &mut [bool],
) -> bool {
    if try_send(client_fd, first_idx, controls, reconnect_needed) {
        return true;
    }

    for offset in 1..controls.len() {
        let idx = (first_idx + offset) % controls.len();
        if try_send(client_fd, idx, controls, reconnect_needed) {
            return true;
        }
    }
    false
}

fn try_send(
    client_fd: RawFd,
    idx: usize,
    controls: &mut [Option<ControlChannel>],
    reconnect_needed: &mut [bool],
) -> bool {
    let Some(control) = controls[idx].as_mut() else {
        reconnect_needed[idx] = true;
        return false;
    };

    const HANDOFF_RETRY: Duration = Duration::from_micros(200);

    let start = std::time::Instant::now();
    loop {
        match control.send_fd(client_fd) {
            Ok(()) => return true,
            Err(err) if is_would_block(&err) => {
                if start.elapsed() >= HANDOFF_RETRY {
                    return false;
                }
                thread::yield_now();
            }
            Err(_) => {
                controls[idx] = None;
                reconnect_needed[idx] = true;
                return false;
            }
        }
    }
}

fn is_would_block(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EAGAIN)
}

fn make_listener(config: &Config) -> io::Result<std::net::TcpListener> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
        set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1)?;
        if config.tcp_defer_accept > 0 {
            let _ = set_int_sockopt(fd, SOL_TCP, TCP_DEFER_ACCEPT, config.tcp_defer_accept);
        }
        if config.busy_poll_us > 0 {
            let _ = set_int_sockopt(fd, libc::SOL_SOCKET, SO_BUSY_POLL, config.busy_poll_us);
        }
        if config.prefer_busy_poll {
            let _ = set_int_sockopt(fd, libc::SOL_SOCKET, SO_PREFER_BUSY_POLL, 1);
        }
        if config.busy_poll_budget > 0 {
            let _ = set_int_sockopt(
                fd,
                libc::SOL_SOCKET,
                SO_BUSY_POLL_BUDGET,
                config.busy_poll_budget,
            );
        }

        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as u16,
            sin_port: config.port.to_be(),
            sin_addr: libc::in_addr { s_addr: 0 },
            sin_zero: [0; 8],
        };

        let bind_result = unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if bind_result != 0 {
            return Err(io::Error::last_os_error());
        }

        let listen_result = unsafe { libc::listen(fd, BACKLOG) };
        if listen_result != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    })();

    if let Err(e) = result {
        unsafe {
            libc::close(fd);
        }
        return Err(e);
    }

    Ok(unsafe { std::net::TcpListener::from_raw_fd(fd) })
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

fn set_int_sockopt(fd: RawFd, level: i32, optname: i32, value: i32) -> io::Result<()> {
    let opt: libc::c_int = value;
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            optname,
            &opt as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn configure_tcp(fd: RawFd) {
    let _ = set_int_sockopt(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, 1);
    let _ = set_int_sockopt(fd, libc::IPPROTO_TCP, libc::TCP_QUICKACK, 1);
}

fn spawn_self_warm(port: u16, requests: usize) {
    if requests == 0 {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        for _ in 0..requests {
            let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            if stream.write_all(WARM_HTTP_HEAD).is_ok() && stream.write_all(WARM_HTTP_BODY).is_ok()
            {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
            }
        }
    });
}

fn connect_control(path: &str) -> Option<ControlChannel> {
    let stream = UnixStream::connect(path).ok()?;
    let fd = stream.as_raw_fd();
    set_nonblocking(fd).ok()?;
    let _ = set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, CONTROL_SNDBUF);

    let mut control = [0u8; 64];
    let cmsg_len;
    unsafe {
        let hdr = control.as_mut_ptr().cast::<libc::cmsghdr>();
        (*hdr).cmsg_len =
            (std::mem::size_of::<libc::cmsghdr>() + std::mem::size_of::<RawFd>()) as _;
        (*hdr).cmsg_level = libc::SOL_SOCKET;
        (*hdr).cmsg_type = libc::SCM_RIGHTS;
        cmsg_len = (*hdr).cmsg_len as usize;
    }

    Some(ControlChannel {
        stream,
        byte: 0,
        control,
        cmsg_len,
    })
}

fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}
