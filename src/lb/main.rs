use std::io;
use std::os::fd::RawFd;
use std::thread;
use std::time::Duration;

const STANDARD_PORT: u16 = 9999;
const DEFAULT_QUEUE_DEPTH: i32 = 65_535;
const DEFAULT_BATCH_LIMIT: usize = 128;
const DEFAULT_UPSTREAM_PATHS: &str = "/sockets/api1.sock,/sockets/api2.sock";
const MAX_UPSTREAM_NODES: usize = 32;
const UNIX_SEND_BUFFER: i32 = 256 * 1024;

struct ProxySettings {
    listen_port: u16,
    queue_depth: i32,
    batch_limit: usize,
    upstream_paths: Vec<String>,
}

impl ProxySettings {
    fn load_from_environment() -> Self {
        let upstream_paths = std::env::var("API_SOCKETS")
            .unwrap_or_else(|_| DEFAULT_UPSTREAM_PATHS.to_string())
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .take(MAX_UPSTREAM_NODES)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        Self {
            listen_port: parse_env_u16("LB_PORT").unwrap_or(STANDARD_PORT),
            queue_depth: parse_env_i32("LB_BACKLOG").unwrap_or(DEFAULT_QUEUE_DEPTH),
            batch_limit: parse_env_usize("LB_ACCEPT_BATCH").unwrap_or(DEFAULT_BATCH_LIMIT),
            upstream_paths,
        }
    }
}

struct UpstreamNode {
    socket_path: String,
    conn_fd: RawFd,
    signal_byte: u8,
    data_vec: libc::iovec,
    ctl_buffer: [u8; 64],
    message_header: libc::msghdr,
    ctl_message: *mut libc::cmsghdr,
}

impl UpstreamNode {
    fn establish_link(socket_path: String) -> Self {
        await_socket_file(&socket_path);
        let conn_fd = loop {
            match open_unix_stream(&socket_path) {
                Ok(fd) => break fd,
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        let mut node = Self {
            socket_path,
            conn_fd,
            signal_byte: 1,
            data_vec: libc::iovec {
                iov_base: std::ptr::null_mut(),
                iov_len: 0,
            },
            ctl_buffer: [0; 64],
            message_header: unsafe { std::mem::zeroed() },
            ctl_message: std::ptr::null_mut(),
        };
        node.build_message_template();
        eprintln!("[lb] connected {}", node.socket_path);
        node
    }

    fn restore_connection(&mut self) {
        unsafe { libc::close(self.conn_fd) };
        await_socket_file(&self.socket_path);
        self.conn_fd = loop {
            match open_unix_stream(&self.socket_path) {
                Ok(fd) => break fd,
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        self.build_message_template();
        eprintln!("[lb] reconnected {}", self.socket_path);
    }

    fn build_message_template(&mut self) {
        self.data_vec = libc::iovec {
            iov_base: (&mut self.signal_byte as *mut u8).cast(),
            iov_len: 1,
        };
        self.message_header = unsafe { std::mem::zeroed() };
        self.message_header.msg_iov = &mut self.data_vec;
        self.message_header.msg_iovlen = 1;
        self.message_header.msg_control = self.ctl_buffer.as_mut_ptr().cast();
        self.message_header.msg_controllen = self.ctl_buffer.len();
        self.ctl_message = unsafe { libc::CMSG_FIRSTHDR(&self.message_header) };
        if !self.ctl_message.is_null() {
            unsafe {
                (*self.ctl_message).cmsg_level = libc::SOL_SOCKET;
                (*self.ctl_message).cmsg_type = libc::SCM_RIGHTS;
                (*self.ctl_message).cmsg_len =
                    libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            }
        }
    }

    fn transfer_descriptor(&mut self, peer_fd: RawFd, async_mode: bool) -> io::Result<()> {
        if self.ctl_message.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "missing cmsg"));
        }
        unsafe {
            let data = libc::CMSG_DATA(self.ctl_message).cast::<RawFd>();
            *data = peer_fd;
        }
        self.message_header.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize };
        let flags = libc::MSG_NOSIGNAL | if async_mode { libc::MSG_DONTWAIT } else { 0 };
        loop {
            let sent = unsafe { libc::sendmsg(self.conn_fd, &self.message_header, flags) };
            if sent > 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
    }
}

impl Drop for UpstreamNode {
    fn drop(&mut self) {
        unsafe { libc::close(self.conn_fd) };
    }
}

fn main() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let settings = ProxySettings::load_from_environment();
    if settings.upstream_paths.is_empty() {
        eprintln!("[lb] API_SOCKETS has no backends");
        std::process::exit(2);
    }

    let mut upstream_pool = settings
        .upstream_paths
        .iter()
        .cloned()
        .map(UpstreamNode::establish_link)
        .collect::<Vec<_>>();
    for node in &mut upstream_pool {
        node.build_message_template();
    }
    let tcp_socket = create_tcp_listener(settings.listen_port, settings.queue_depth)
        .unwrap_or_else(|err| {
            eprintln!(
                "[lb] failed to listen on :{}: {}",
                settings.listen_port, err
            );
            std::process::exit(3);
        });

    eprintln!(
        "[lb] listening :{} backlog={} batch={} backends={}",
        settings.listen_port,
        settings.queue_depth,
        settings.batch_limit,
        upstream_pool.len()
    );

    let mut rotation_cursor = 0usize;
    loop {
        let mut processed_count = 0usize;
        while processed_count < settings.batch_limit {
            let peer_fd = match accept_incoming(tcp_socket) {
                Ok(Some(fd)) => fd,
                Ok(None) => break,
                Err(_) => break,
            };
            processed_count += 1;
            configure_client_socket(peer_fd);

            let start_cursor = rotation_cursor;
            rotation_cursor = (rotation_cursor + 1) % upstream_pool.len();
            let mut dispatched = false;
            for step in 0..upstream_pool.len() {
                let position = (start_cursor + step) % upstream_pool.len();
                if route_to_upstream(&mut upstream_pool[position], peer_fd, true).is_ok() {
                    dispatched = true;
                    break;
                }
            }
            if !dispatched {
                let _ = route_to_upstream(&mut upstream_pool[start_cursor], peer_fd, false);
            }
            unsafe { libc::close(peer_fd) };
        }

        if processed_count == 0 {
            block_until_readable(tcp_socket);
        }
    }
}

fn route_to_upstream(node: &mut UpstreamNode, peer_fd: RawFd, async_mode: bool) -> io::Result<()> {
    match node.transfer_descriptor(peer_fd, async_mode) {
        Ok(()) => Ok(()),
        Err(err) => {
            if async_mode && err.raw_os_error() == Some(libc::EAGAIN) {
                return Err(err);
            }
            node.restore_connection();
            node.transfer_descriptor(peer_fd, async_mode)
        }
    }
}

fn create_tcp_listener(port: u16, queue_depth: i32) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::socket(
            libc::AF_INET,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        apply_socket_option(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
        apply_socket_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1)?;
        let _ = apply_socket_option(fd, libc::IPPROTO_TCP, 9, 1);

        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr {
                s_addr: libc::INADDR_ANY.to_be(),
            },
            sin_zero: [0; 8],
        };
        let rc = unsafe {
            libc::bind(
                fd,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::listen(fd, queue_depth) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();

    if let Err(err) = result {
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

fn accept_incoming(listener: RawFd) -> io::Result<Option<RawFd>> {
    let fd = unsafe {
        libc::accept4(
            listener,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
        )
    };
    if fd >= 0 {
        return Ok(Some(fd));
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => Ok(None),
        Some(libc::EINTR) => Ok(None),
        _ => Err(err),
    }
}

fn open_unix_stream(path: &str) -> io::Result<RawFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        apply_socket_option(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, UNIX_SEND_BUFFER)?;
        let addr = build_unix_address(path)?;
        let rc = unsafe {
            libc::connect(
                fd,
                &addr.storage as *const _ as *const libc::sockaddr,
                addr.len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if let Err(err) = result {
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

fn await_socket_file(path: &str) {
    for _ in 0..600 {
        if std::path::Path::new(path).exists() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn block_until_readable(fd: RawFd) {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe {
        libc::poll(&mut pfd, 1, -1);
    }
}

fn configure_client_socket(fd: RawFd) {
    let _ = apply_socket_option(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, 1);
    let _ = apply_socket_option(fd, libc::IPPROTO_TCP, libc::TCP_QUICKACK, 1);
}

fn apply_socket_option(fd: RawFd, level: i32, optname: i32, value: i32) -> io::Result<()> {
    let opt: libc::c_int = value;
    let rc = unsafe {
        libc::setsockopt(
            fd,
            level,
            optname,
            &opt as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct UnixAddress {
    storage: libc::sockaddr_un,
    len: libc::socklen_t,
}

fn build_unix_address(path: &str) -> io::Result<UnixAddress> {
    let bytes = path.as_bytes();
    if bytes.is_empty() || bytes.len() >= 108 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unix socket path is empty or too long",
        ));
    }

    let mut storage: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    storage.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in storage.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *dst = src as libc::c_char;
    }
    Ok(UnixAddress {
        storage,
        len: (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t,
    })
}

fn parse_env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok()?.parse().ok()
}

fn parse_env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.parse().ok()
}

fn parse_env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}
