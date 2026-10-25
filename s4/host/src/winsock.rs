//! Winsock 1.1/2 on the host's BSD sockets. Addresses (`sockaddr_in`) have the
//! same layout; constants and error numbers are translated.

use crate::Shim;
use s4_x86::Cpu;
use std::cell::Cell;

macro_rules! sh {
    ($name:expr, $args:expr, $f:expr) => {
        Shim { dll: "ws2_32.dll", name: $name, args: $args, cdecl: false, f: $f }
    };
}

thread_local! {
    /// UDP sockets the game bound to a specific local address: fd -> that address.
    static BOUND_IP: std::cell::RefCell<std::collections::HashMap<i32, [u8; 4]>> = Default::default();

    static WSA_ERROR: Cell<u32> = const { Cell::new(0) };
}

fn wsa_error() -> u32 {
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let w = match e {
        libc::EWOULDBLOCK => 10035,
        libc::EINPROGRESS => 10036,
        libc::EALREADY => 10037,
        libc::ENOTSOCK => 10038,
        libc::EMSGSIZE => 10040,
        libc::EPROTOTYPE => 10041,
        libc::ENOPROTOOPT => 10042,
        libc::EPROTONOSUPPORT => 10043,
        libc::EOPNOTSUPP => 10045,
        libc::EAFNOSUPPORT => 10047,
        libc::EADDRINUSE => 10048,
        libc::EADDRNOTAVAIL => 10049,
        libc::ENETUNREACH => 10051,
        libc::ECONNABORTED => 10053,
        libc::ECONNRESET => 10054,
        libc::ENOBUFS => 10055,
        libc::EISCONN => 10056,
        libc::ENOTCONN => 10057,
        libc::ETIMEDOUT => 10060,
        libc::ECONNREFUSED => 10061,
        libc::EHOSTUNREACH => 10065,
        libc::EINVAL => 10022,
        libc::EBADF => 10009,
        libc::EACCES => 10013,
        libc::EFAULT => 10014,
        _ => 10000 + e as u32,
    };
    WSA_ERROR.with(|c| c.set(w));
    w
}

fn check(r: i32) -> u32 {
    if r < 0 {
        wsa_error();
        0xffff_ffff
    } else {
        r as u32
    }
}

fn sockaddr(c: &Cpu, p: u32, len: u32) -> Vec<u8> {
    c.mem.read_bytes(p, len as usize)
}

fn opt_level(level: u32, name: u32) -> (i32, i32) {
    if level == 0xffff {
        let n = match name {
            4 => libc::SO_REUSEADDR,
            8 => libc::SO_KEEPALIVE,
            0x20 => libc::SO_BROADCAST,
            0x80 => libc::SO_LINGER,
            0x1001 => libc::SO_SNDBUF,
            0x1002 => libc::SO_RCVBUF,
            0x1007 => libc::SO_ERROR,
            0x1008 => libc::SO_TYPE,
            _ => name as i32,
        };
        (libc::SOL_SOCKET, n)
    } else if level == 6 {
        (libc::IPPROTO_TCP, name as i32)
    } else {
        (level as i32, name as i32)
    }
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return "localhost".into();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The address of the interface that routes to the LAN/internet (no packet is sent), else loopback.
fn local_ip() -> std::net::Ipv4Addr {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok();
    match sock.as_ref().and_then(|s| s.connect("192.0.2.1:9").ok().and_then(|_| s.local_addr().ok())) {
        Some(std::net::SocketAddr::V4(v)) => *v.ip(),
        _ => std::net::Ipv4Addr::LOCALHOST,
    }
}

pub fn all() -> Vec<Shim> {
    vec![
        sh!("WSAStartup", 2, |c, a| {
            // WSADATA: version 2.2, high version 2.2, description.
            c.mem.fill(a[1], 0, 400);
            c.mem.w16(a[1], 0x0202);
            c.mem.w16(a[1] + 2, 0x0202);
            0
        }),
        sh!("WSACleanup", 0, |_, _| 0),
        sh!("WSAGetLastError", 0, |_, _| WSA_ERROR.with(|c| c.get())),
        sh!("socket", 3, |_, a| check(unsafe { libc::socket(a[0] as i32, a[1] as i32, a[2] as i32) })),
        sh!("closesocket", 1, |_, a| {
            BOUND_IP.with(|m| m.borrow_mut().remove(&(a[0] as i32)));
            check(unsafe { libc::close(a[0] as i32) })
        }),
        sh!("bind", 3, |c, a| {
            let sa = sockaddr(c, a[1], a[2]);
            if std::env::var_os("S4_HOST_TRACE").is_some() {
                eprintln!("  bind {:02x?}", &sa[..sa.len().min(8)]);
            }
            let mut sa = sa;
            // A Windows UDP socket bound to a specific local address still receives broadcasts to its
            // port; Linux's does not. Bind to the wildcard and remember the address for getsockname.
            if sa.len() >= 8 && sa[0] == 2 && sa[4] != 0 && sa[4] != 127 {
                let mut ty = 0i32;
                let mut l = 4 as libc::socklen_t;
                // SAFETY: valid out-parameters.
                unsafe { libc::getsockopt(a[0] as i32, libc::SOL_SOCKET, libc::SO_TYPE, (&mut ty as *mut i32).cast(), &mut l) };
                if ty == libc::SOCK_DGRAM {
                    BOUND_IP.with(|m| m.borrow_mut().insert(a[0] as i32, [sa[4], sa[5], sa[6], sa[7]]));
                    sa[4..8].fill(0);
                }
            }
            check(unsafe { libc::bind(a[0] as i32, sa.as_ptr().cast(), a[2]) })
        }),
        sh!("connect", 3, |c, a| {
            let sa = sockaddr(c, a[1], a[2]);
            check(unsafe { libc::connect(a[0] as i32, sa.as_ptr().cast(), a[2]) })
        }),
        sh!("listen", 2, |_, a| check(unsafe { libc::listen(a[0] as i32, a[1] as i32) })),
        sh!("accept", 3, |c, a| {
            let mut buf = [0u8; 128];
            let mut len: libc::socklen_t = if a[2] != 0 { c.mem.r32(a[2]) } else { 16 };
            let r = unsafe { libc::accept(a[0] as i32, buf.as_mut_ptr().cast(), &mut len) };
            if r >= 0 && a[1] != 0 {
                c.mem.write_bytes(a[1], &buf[..len as usize]);
                c.mem.w32(a[2], len);
            }
            check(r)
        }),
        sh!("send", 4, |c, a| {
            let data = c.mem.read_bytes(a[1], a[2] as usize);
            check(unsafe { libc::send(a[0] as i32, data.as_ptr().cast(), data.len(), a[3] as i32 | libc::MSG_NOSIGNAL) } as i32)
        }),
        sh!("sendto", 6, |c, a| {
            let data = c.mem.read_bytes(a[1], a[2] as usize);
            let sa = sockaddr(c, a[4], a[5]);
            if std::env::var_os("S4_HOST_TRACE").is_some() {
                eprintln!("  sendto {} bytes to {:?}: {:02x?}", data.len(), &sa[..sa.len().min(8)], &data[..data.len().min(16)]);
            }
            check(unsafe { libc::sendto(a[0] as i32, data.as_ptr().cast(), data.len(), a[3] as i32 | libc::MSG_NOSIGNAL, sa.as_ptr().cast(), a[5]) } as i32)
        }),
        sh!("recv", 4, |c, a| {
            let mut buf = vec![0u8; a[2] as usize];
            let r = unsafe { libc::recv(a[0] as i32, buf.as_mut_ptr().cast(), buf.len(), a[3] as i32) } as i32;
            if r > 0 {
                c.mem.write_bytes(a[1], &buf[..r as usize]);
            }
            check(r)
        }),
        sh!("recvfrom", 6, |c, a| {
            let mut buf = vec![0u8; a[2] as usize];
            let mut sa = [0u8; 128];
            let mut len: libc::socklen_t = if a[5] != 0 { c.mem.r32(a[5]) } else { 16 };
            let r = unsafe { libc::recvfrom(a[0] as i32, buf.as_mut_ptr().cast(), buf.len(), a[3] as i32, sa.as_mut_ptr().cast(), &mut len) } as i32;
            if r >= 0 {
                c.mem.write_bytes(a[1], &buf[..r as usize]);
                if a[4] != 0 {
                    c.mem.write_bytes(a[4], &sa[..len as usize]);
                    c.mem.w32(a[5], len);
                }
            }
            check(r)
        }),
        sh!("shutdown", 2, |_, a| check(unsafe { libc::shutdown(a[0] as i32, a[1] as i32) })),
        sh!("getsockname", 3, |c, a| {
            let mut buf = [0u8; 128];
            let mut len: libc::socklen_t = c.mem.r32(a[2]);
            let r = unsafe { libc::getsockname(a[0] as i32, buf.as_mut_ptr().cast(), &mut len) };
            if r == 0 {
                if len >= 8 && buf[0] == 2 && buf[4..8] == [0, 0, 0, 0] {
                    if let Some(ip) = BOUND_IP.with(|m| m.borrow().get(&(a[0] as i32)).copied()) {
                        buf[4..8].copy_from_slice(&ip);
                    }
                }
                c.mem.write_bytes(a[1], &buf[..len as usize]);
                c.mem.w32(a[2], len);
            }
            check(r)
        }),
        sh!("getpeername", 3, |c, a| {
            let mut buf = [0u8; 128];
            let mut len: libc::socklen_t = c.mem.r32(a[2]);
            let r = unsafe { libc::getpeername(a[0] as i32, buf.as_mut_ptr().cast(), &mut len) };
            if r == 0 {
                c.mem.write_bytes(a[1], &buf[..len as usize]);
                c.mem.w32(a[2], len);
            }
            check(r)
        }),
        sh!("setsockopt", 5, |c, a| {
            let (l, n) = opt_level(a[1], a[2]);
            let v = c.mem.read_bytes(a[3], a[4] as usize);
            check(unsafe { libc::setsockopt(a[0] as i32, l, n, v.as_ptr().cast(), a[4]) })
        }),
        sh!("getsockopt", 5, |c, a| {
            let (l, n) = opt_level(a[1], a[2]);
            let mut buf = [0u8; 64];
            let mut len: libc::socklen_t = c.mem.r32(a[4]);
            let r = unsafe { libc::getsockopt(a[0] as i32, l, n, buf.as_mut_ptr().cast(), &mut len) };
            if r == 0 {
                c.mem.write_bytes(a[3], &buf[..len as usize]);
                c.mem.w32(a[4], len);
            }
            check(r)
        }),
        sh!("ioctlsocket", 3, |c, a| {
            // FIONBIO = 0x8004667e
            if a[1] == 0x8004_667e {
                let on = c.mem.r32(a[2]) != 0;
                let fl = unsafe { libc::fcntl(a[0] as i32, libc::F_GETFL) };
                let nf = if on { fl | libc::O_NONBLOCK } else { fl & !libc::O_NONBLOCK };
                return check(unsafe { libc::fcntl(a[0] as i32, libc::F_SETFL, nf) });
            }
            if a[1] == 0x4004_667f {
                // FIONREAD
                let mut n: i32 = 0;
                let r = unsafe { libc::ioctl(a[0] as i32, libc::FIONREAD, &mut n) };
                c.mem.w32(a[2], n as u32);
                return check(r);
            }
            0
        }),
        sh!("select", 5, |c, a| {
            let read = |p: u32| -> Vec<i32> {
                if p == 0 {
                    return Vec::new();
                }
                (0..c.mem.r32(p)).map(|i| c.mem.r32(p + 4 + 4 * i) as i32).collect()
            };
            let (r, w, e) = (read(a[1]), read(a[2]), read(a[3]));
            let mut set = |v: &[i32]| -> (libc::fd_set, i32) {
                let mut s: libc::fd_set = unsafe { std::mem::zeroed() };
                unsafe { libc::FD_ZERO(&mut s) };
                let mut m = -1;
                for &f in v {
                    unsafe { libc::FD_SET(f, &mut s) };
                    m = m.max(f);
                }
                (s, m)
            };
            let ((mut rs, rm), (mut ws, wm), (mut es, em)) = (set(&r), set(&w), set(&e));
            let mut tv;
            let tvp = if a[4] != 0 {
                tv = libc::timeval { tv_sec: c.mem.r32(a[4]) as _, tv_usec: c.mem.r32(a[4] + 4) as _ };
                &mut tv as *mut libc::timeval
            } else {
                std::ptr::null_mut()
            };
            let n = unsafe { libc::select(rm.max(wm).max(em) + 1, if a[1] != 0 { &mut rs } else { std::ptr::null_mut() }, if a[2] != 0 { &mut ws } else { std::ptr::null_mut() }, if a[3] != 0 { &mut es } else { std::ptr::null_mut() }, tvp) };
            let write_back = |p: u32, v: &[i32], s: &libc::fd_set| {
                if p == 0 {
                    return;
                }
                let ready: Vec<i32> = v.iter().copied().filter(|&f| unsafe { libc::FD_ISSET(f, s) }).collect();
                c.mem.w32(p, ready.len() as u32);
                for (i, f) in ready.iter().enumerate() {
                    c.mem.w32(p + 4 + 4 * i as u32, *f as u32);
                }
            };
            if n >= 0 {
                write_back(a[1], &r, &rs);
                write_back(a[2], &w, &ws);
                write_back(a[3], &e, &es);
            }
            check(n)
        }),
        sh!("htons", 1, |_, a| (a[0] as u16).swap_bytes() as u32),
        sh!("ntohs", 1, |_, a| (a[0] as u16).swap_bytes() as u32),
        sh!("htonl", 1, |_, a| a[0].swap_bytes()),
        sh!("ntohl", 1, |_, a| a[0].swap_bytes()),
        sh!("inet_addr", 1, |c, a| {
            let s: String = c.mem.cstr(a[0]).iter().map(|&b| b as char).collect();
            match s.parse::<std::net::Ipv4Addr>() {
                Ok(ip) => u32::from_le_bytes(ip.octets()),
                Err(_) => 0xffff_ffff,
            }
        }),
        sh!("inet_ntoa", 1, |c, a| {
            let ip = a[0].to_le_bytes();
            let s = format!("{}.{}.{}.{}\0", ip[0], ip[1], ip[2], ip[3]);
            let p = crate::heap::alloc(c, 20);
            c.mem.write_bytes(p, s.as_bytes());
            p
        }),
        sh!("gethostname", 2, |c, a| {
            let mut name = host_name().into_bytes();
            name.push(0);
            if a[1] < name.len() as u32 {
                return 0xffff_ffff;
            }
            c.mem.write_bytes(a[0], &name);
            0
        }),
        sh!("gethostbyname", 1, |c, a| {
            let name: String = c.mem.cstr(a[0]).iter().map(|&b| b as char).collect();
            use std::net::ToSocketAddrs;
            let ip = match name.parse::<std::net::Ipv4Addr>() {
                Ok(ip) => Some(ip),
                // The machine's own name: its LAN address (what the game advertises to other players).
                Err(_) if name.eq_ignore_ascii_case(&host_name()) => Some(local_ip()),
                Err(_) => (name.as_str(), 0).to_socket_addrs().ok().and_then(|mut it| it.find_map(|s| if let std::net::SocketAddr::V4(v) = s { Some(*v.ip()) } else { None })),
            };
            let Some(ip) = ip else {
                WSA_ERROR.with(|c| c.set(11001));
                return 0;
            };
            // hostent: name, aliases, addrtype, length, addr_list
            let h = crate::heap::alloc_zeroed(c, 64);
            let addr = h + 32;
            c.mem.write_bytes(addr, &ip.octets());
            let list = h + 40;
            c.mem.w32(list, addr);
            c.mem.w32(list + 4, 0);
            c.mem.w32(h + 8, 2);
            c.mem.w16(h + 8, 2);
            c.mem.w16(h + 10, 4);
            c.mem.w32(h + 12, list);
            let nm = h + 48;
            c.mem.write_bytes(nm, b"host\0");
            c.mem.w32(h, nm);
            c.mem.w32(h + 4, h + 56);
            h
        }),
    ]
}
