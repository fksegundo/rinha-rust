use std::io;
use std::os::fd::RawFd;
use std::thread;
use std::time::Duration;

const DEFAULT_PORT: u16 = 9999;
const DEFAULT_BACKLOG: i32 = 65_535;
const DEFAULT_ACCEPT_BATCH: usize = 128;
const DEFAULT_API_SOCKETS: &str = "/sockets/api1.sock,/sockets/api2.sock";
const MAX_BACKENDS: usize = 32;
const CONTROL_SNDBUF: i32 = 256 * 1024;

struct Config {
    port: u16,
    backlog: i32,
    accept_batch: usize,
    backends: Vec<String>,
}

impl Config {
    fn from_env() -> Self {
        let backends = std::env::var("API_SOCKETS")
            .unwrap_or_else(|_| DEFAULT_API_SOCKETS.to_string())
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .take(MAX_BACKENDS)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        Self {
            port: env_u16("LB_PORT").unwrap_or(DEFAULT_PORT),
            backlog: env_i32("LB_BACKLOG").unwrap_or(DEFAULT_BACKLOG),
            accept_batch: env_usize("LB_ACCEPT_BATCH").unwrap_or(DEFAULT_ACCEPT_BATCH),
            backends,
        }
    }
}

struct Backend {
    path: String,
    fd: RawFd,
    byte: u8,
    iov: libc::iovec,
    control: [u8; 64],
    msg: libc::msghdr,
    cmsg: *mut libc::cmsghdr,
}

impl Backend {
    fn connect(path: String) -> Self {
        wait_for_socket(&path);
        let fd = loop {
            match connect_seqpacket(&path) {
                Ok(fd) => break fd,
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        let mut backend = Self {
            path,
            fd,
            byte: 1,
            iov: libc::iovec {
                iov_base: std::ptr::null_mut(),
                iov_len: 0,
            },
            control: [0; 64],
            msg: unsafe { std::mem::zeroed() },
            cmsg: std::ptr::null_mut(),
        };
        backend.init_msg();
        eprintln!("[lb] connected {}", backend.path);
        backend
    }

    fn reconnect(&mut self) {
        unsafe { libc::close(self.fd) };
        wait_for_socket(&self.path);
        self.fd = loop {
            match connect_seqpacket(&self.path) {
                Ok(fd) => break fd,
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        self.init_msg();
        eprintln!("[lb] reconnected {}", self.path);
    }

    fn init_msg(&mut self) {
        self.iov = libc::iovec {
            iov_base: (&mut self.byte as *mut u8).cast(),
            iov_len: 1,
        };
        self.msg = unsafe { std::mem::zeroed() };
        self.msg.msg_iov = &mut self.iov;
        self.msg.msg_iovlen = 1;
        self.msg.msg_control = self.control.as_mut_ptr().cast();
        self.msg.msg_controllen = self.control.len();
        self.cmsg = unsafe { libc::CMSG_FIRSTHDR(&self.msg) };
        if !self.cmsg.is_null() {
            unsafe {
                (*self.cmsg).cmsg_level = libc::SOL_SOCKET;
                (*self.cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*self.cmsg).cmsg_len =
                    libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            }
        }
    }

    fn send_fd(&mut self, client_fd: RawFd, nonblocking: bool) -> io::Result<()> {
        if self.cmsg.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "missing cmsg"));
        }
        unsafe {
            let data = libc::CMSG_DATA(self.cmsg).cast::<RawFd>();
            *data = client_fd;
        }
        self.msg.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize };
        let flags = libc::MSG_NOSIGNAL | if nonblocking { libc::MSG_DONTWAIT } else { 0 };
        loop {
            let sent = unsafe { libc::sendmsg(self.fd, &self.msg, flags) };
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

impl Drop for Backend {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

fn main() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let config = Config::from_env();
    if config.backends.is_empty() {
        eprintln!("[lb] API_SOCKETS has no backends");
        std::process::exit(2);
    }

    let mut backends = config
        .backends
        .iter()
        .cloned()
        .map(Backend::connect)
        .collect::<Vec<_>>();
    for backend in &mut backends {
        backend.init_msg();
    }
    let listener = listen_tcp(config.port, config.backlog).unwrap_or_else(|err| {
        eprintln!("[lb] failed to listen on :{}: {}", config.port, err);
        std::process::exit(3);
    });

    eprintln!(
        "[lb] listening :{} backlog={} batch={} backends={}",
        config.port,
        config.backlog,
        config.accept_batch,
        backends.len()
    );

    let mut rr = 0usize;
    loop {
        let mut accepted = 0usize;
        while accepted < config.accept_batch {
            let client_fd = match accept_client(listener) {
                Ok(Some(fd)) => fd,
                Ok(None) => break,
                Err(_) => break,
            };
            accepted += 1;
            tune_client(client_fd);

            let first = rr;
            rr = (rr + 1) % backends.len();
            let mut ok = false;
            for offset in 0..backends.len() {
                let idx = (first + offset) % backends.len();
                if send_to_backend(&mut backends[idx], client_fd, true).is_ok() {
                    ok = true;
                    break;
                }
            }
            if !ok {
                let _ = send_to_backend(&mut backends[first], client_fd, false);
            }
            unsafe { libc::close(client_fd) };
        }

        if accepted == 0 {
            wait_read(listener);
        }
    }
}

fn send_to_backend(backend: &mut Backend, client_fd: RawFd, nonblocking: bool) -> io::Result<()> {
    match backend.send_fd(client_fd, nonblocking) {
        Ok(()) => Ok(()),
        Err(err) => {
            if nonblocking && err.raw_os_error() == Some(libc::EAGAIN) {
                return Err(err);
            }
            backend.reconnect();
            backend.send_fd(client_fd, nonblocking)
        }
    }
}

fn listen_tcp(port: u16, backlog: i32) -> io::Result<RawFd> {
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
        set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
        set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1)?;
        let _ = set_int_sockopt(fd, libc::IPPROTO_TCP, 9, 1);

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
        if unsafe { libc::listen(fd, backlog) } != 0 {
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

fn accept_client(listener: RawFd) -> io::Result<Option<RawFd>> {
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

fn connect_seqpacket(path: &str) -> io::Result<RawFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        set_int_sockopt(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, CONTROL_SNDBUF)?;
        let addr = unix_sockaddr(path)?;
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

fn wait_for_socket(path: &str) {
    for _ in 0..600 {
        if std::path::Path::new(path).exists() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_read(fd: RawFd) {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe {
        libc::poll(&mut pfd, 1, -1);
    }
}

fn tune_client(fd: RawFd) {
    let _ = set_int_sockopt(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, 1);
    let _ = set_int_sockopt(fd, libc::IPPROTO_TCP, libc::TCP_QUICKACK, 1);
}

fn set_int_sockopt(fd: RawFd, level: i32, optname: i32, value: i32) -> io::Result<()> {
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

struct UnixSockAddr {
    storage: libc::sockaddr_un,
    len: libc::socklen_t,
}

fn unix_sockaddr(path: &str) -> io::Result<UnixSockAddr> {
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
    Ok(UnixSockAddr {
        storage,
        len: (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t,
    })
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}
