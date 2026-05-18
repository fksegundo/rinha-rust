use std::net::TcpStream;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixListener;
use std::sync::Arc;

pub fn run_fd_server<F>(socket_path: &str, pool_size: usize, handler: F)
where
    F: Fn(TcpStream) + Send + Sync + 'static,
{
    let _ = std::fs::remove_file(socket_path);
    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind fd socket {}: {}", socket_path, e);
            return;
        }
    };
    let handler = Arc::new(handler);
    let pool = threadpool::Builder::new()
        .num_threads(pool_size)
        .thread_stack_size(256 * 1024)
        .build();

    for stream in listener.incoming() {
        match stream {
            Ok(control) => {
                let handler = Arc::clone(&handler);
                let pool = pool.clone();
                std::thread::spawn(move || {
                    let mut control = control;
                    loop {
                        let fd = match recv_fd(&mut control) {
                            Some(fd) => fd,
                            None => break,
                        };
                        let tcp = unsafe { TcpStream::from_raw_fd(fd) };
                        let _ = tcp.set_nodelay(true);
                        let handler = Arc::clone(&handler);
                        pool.execute(move || {
                            handler(tcp);
                        });
                    }
                });
            }
            Err(e) => {
                eprintln!("fd accept error: {}", e);
            }
        }
    }
}

fn recv_fd(stream: &mut std::os::unix::net::UnixStream) -> Option<i32> {
    let mut buf = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u8; 64];
    let mut msg = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len() as _,
        msg_flags: 0,
    };

    let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if received < 0 {
        return None;
    }

    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg) as *const i32;
                return Some(*data);
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    None
}
