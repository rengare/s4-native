//! DirectPlay 4 (ANSI) for the games that find and join each other through it, over the local network.
//!
//! A host (`Open` with DPOPEN_CREATE) answers session searches on UDP port 47624 and accepts TCP connections; the
//! other nodes (`Open` with DPOPEN_JOIN) connect to it. The host owns the player table and relays the messages
//! (a star: every node has one connection, to the host). System messages (a player appeared or went away, the
//! session was lost) are queued for the local players like the real thing.

use crate::ddraw::{add_ref, make_vtable, new_object, release};
use crate::handles::{self, Obj};
use crate::heap;
use s4_x86::Cpu;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const DISCOVERY_PORT: u16 = 47624;

const fn dperr(code: u32) -> u32 {
    0x8877_0000 | code
}
const E_NOTIMPL: u32 = 0x8000_4001;
const E_INVALIDARG: u32 = 0x8007_0057;
const DPERR_ALREADYINITIALIZED: u32 = dperr(5);
const DPERR_BUFFERTOOSMALL: u32 = dperr(30);
const DPERR_CANTADDPLAYER: u32 = dperr(40);
const DPERR_CANTCREATEPLAYER: u32 = dperr(60);
const DPERR_CANTCREATESESSION: u32 = dperr(70);
const DPERR_INVALIDPLAYER: u32 = dperr(150);
const DPERR_NOCONNECTION: u32 = dperr(170);
const DPERR_NOMESSAGES: u32 = dperr(190);
const DPERR_NOSESSIONS: u32 = dperr(220);
const DPERR_SENDTOOBIG: u32 = dperr(230);
const DPERR_ACCESSDENIED: u32 = dperr(10);
const DPERR_INVALIDPASSWORD: u32 = dperr(100);

const DPSYS_CREATEPLAYERORGROUP: u32 = 0x0003;
const DPSYS_DESTROYPLAYERORGROUP: u32 = 0x0005;
const DPSYS_SESSIONLOST: u32 = 0x0031;
const DPSYS_SETPLAYERORGROUPDATA: u32 = 0x0102;
const DPSYS_SETSESSIONDESC: u32 = 0x0104;

const DPENUMPLAYERS_LOCAL: u32 = 0x08;
const DPENUMPLAYERS_REMOTE: u32 = 0x10;
const DPENUMSESSIONS_AVAILABLE: u32 = 1;
const DPENUMSESSIONS_ASYNC: u32 = 0x10;
const DPENUMSESSIONS_STOPASYNC: u32 = 0x20;
const DPENUMSESSIONS_PASSWORDREQUIRED: u32 = 0x40;
const DPSESSION_NEWPLAYERSDISABLED: u32 = 1;
const DPSESSION_PASSWORDREQUIRED: u32 = 0x1000_0000;
const DPOPEN_JOIN: u32 = 1;
const DPOPEN_CREATE: u32 = 2;
const DPRECEIVE_TOPLAYER: u32 = 2;
const DPRECEIVE_FROMPLAYER: u32 = 4;
const DPRECEIVE_PEEK: u32 = 0x20;

const TCPIP_SP: [u8; 16] = [0xe0, 0x5e, 0xe9, 0x36, 0x77, 0x85, 0xcf, 0x11, 0x96, 0x0c, 0x00, 0x80, 0xc7, 0x53, 0x4e, 0x82];
const DPAID_SERVICE_PROVIDER: [u8; 16] = [0xc0, 0x16, 0xd9, 0x07, 0xaf, 0xe0, 0xcf, 0x11, 0x9c, 0x4e, 0x00, 0xa0, 0xc9, 0x05, 0x42, 0x5e];

fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("S4_DP_TRACE").is_some())
}

// ---- wire format ----

#[derive(Default)]
struct W(Vec<u8>);
impl W {
    fn u32(&mut self, v: u32) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut W {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn str(&mut self, v: &str) -> &mut W {
        self.bytes(v.as_bytes())
    }
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        v
    }
    fn u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        for x in &mut b {
            *x = self.u8();
        }
        u32::from_le_bytes(b)
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        let s = self.1.min(self.0.len());
        let e = (self.1 + n).min(self.0.len());
        self.1 += n;
        self.0[s..e].to_vec()
    }
    fn str(&mut self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }
}

#[derive(Clone, Default)]
struct Desc {
    flags: u32,
    instance: [u8; 16],
    app: [u8; 16],
    max: u32,
    cur: u32,
    name: String,
    pass: String,
    user: [u32; 4],
}

impl Desc {
    fn enc(&self, w: &mut W) {
        w.u32(self.flags).bytes(&self.instance).bytes(&self.app).u32(self.max).u32(self.cur).str(&self.name).str(&self.pass);
        for u in self.user {
            w.u32(u);
        }
    }
    fn dec(r: &mut R) -> Desc {
        let mut d = Desc { flags: r.u32(), ..Desc::default() };
        d.instance.copy_from_slice(&r.bytes().iter().copied().chain([0; 16]).take(16).collect::<Vec<_>>());
        d.app.copy_from_slice(&r.bytes().iter().copied().chain([0; 16]).take(16).collect::<Vec<_>>());
        d.max = r.u32();
        d.cur = r.u32();
        d.name = r.str();
        d.pass = r.str();
        for u in &mut d.user {
            *u = r.u32();
        }
        d
    }
    fn read_guest(c: &Cpu, p: u32) -> Desc {
        let g = |o: u32| c.mem.r32(p + o);
        let s = |o: u32| if g(o) == 0 { String::new() } else { crate::kernel32::cstr(c, g(o)) };
        let mut d = Desc { flags: g(4), max: g(40), cur: g(44), name: s(48), pass: s(52), user: [g(64), g(68), g(72), g(76)], ..Desc::default() };
        d.instance.copy_from_slice(&c.mem.read_bytes(p + 8, 16));
        d.app.copy_from_slice(&c.mem.read_bytes(p + 24, 16));
        d
    }
    /// DPSESSIONDESC2 (80 bytes) at `at`, followed by the name and the password; returns the size used.
    fn write_guest(&self, c: &mut Cpu, at: u32) -> u32 {
        let m = c.mem;
        let name_at = at + 80;
        let pass_at = name_at + self.name.len() as u32 + 1;
        m.w32(at, 80);
        m.w32(at + 4, self.flags);
        m.write_bytes(at + 8, &self.instance);
        m.write_bytes(at + 24, &self.app);
        m.w32(at + 40, self.max);
        m.w32(at + 44, self.cur);
        m.w32(at + 48, name_at);
        m.w32(at + 52, pass_at);
        m.w32(at + 56, 0);
        m.w32(at + 60, 0);
        for (i, u) in self.user.iter().enumerate() {
            m.w32(at + 64 + 4 * i as u32, *u);
        }
        m.write_bytes(name_at, self.name.as_bytes());
        m.w8(name_at + self.name.len() as u32, 0);
        m.write_bytes(pass_at, self.pass.as_bytes());
        m.w8(pass_at + self.pass.len() as u32, 0);
        self.size()
    }
    fn size(&self) -> u32 {
        80 + self.name.len() as u32 + self.pass.len() as u32 + 2
    }
    fn joinable(&self) -> bool {
        self.flags & DPSESSION_NEWPLAYERSDISABLED == 0 && (self.max == 0 || self.cur < self.max)
    }
}

#[derive(Clone)]
struct Player {
    id: u32,
    node: u32,
    short: String,
    long: String,
    flags: u32,
    data: Vec<u8>,
    event: u32,
}

impl Player {
    fn enc(&self, w: &mut W) {
        w.u32(self.id).u32(self.node).str(&self.short).str(&self.long).u32(self.flags).bytes(&self.data);
    }
    fn dec(r: &mut R) -> Player {
        Player { id: r.u32(), node: r.u32(), short: r.str(), long: r.str(), flags: r.u32(), data: r.bytes(), event: 0 }
    }
}

/// A queued message: pointers in `data` are offsets into it (listed in `relocs`) until it is received.
struct Msg {
    from: u32,
    data: Vec<u8>,
    relocs: Vec<usize>,
}

struct Sess {
    desc: Desc,
    host: bool,
}

#[derive(Default)]
struct Net {
    inited: bool,
    sess: Option<Sess>,
    players: BTreeMap<u32, Player>,
    queues: HashMap<u32, VecDeque<(u32, Msg)>>,
    my_node: u32,
    next_player: u32,
    next_node: u32,
    peers: HashMap<u32, TcpStream>,
    found: Vec<(Desc, SocketAddr)>,
    create_reply: Option<u32>,
    /// Bumped whenever a session starts or ends: the threads of an older one stop.
    epoch: u64,
    lost: bool,
    /// The host address the player typed in (the INet part of the connection address), if any.
    host_ip: String,
}

static NET: Mutex<Option<Net>> = Mutex::new(None);
static CV: Condvar = Condvar::new();

fn net() -> MutexGuard<'static, Option<Net>> {
    let mut g = NET.lock().unwrap();
    g.get_or_insert_with(Net::default);
    g
}

macro_rules! n {
    ($g:expr) => {
        $g.as_mut().unwrap()
    };
}

fn send_frame(s: &mut TcpStream, kind: u8, body: &[u8]) {
    let mut f = Vec::with_capacity(body.len() + 5);
    f.extend_from_slice(&(body.len() as u32 + 1).to_le_bytes());
    f.push(kind);
    f.extend_from_slice(body);
    let _ = s.write_all(&f);
}

fn read_frame(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut l = [0u8; 4];
    s.read_exact(&mut l).ok()?;
    let n = u32::from_le_bytes(l) as usize;
    if n == 0 || n > 1 << 20 {
        return None;
    }
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).ok()?;
    Some((b[0], b[1..].to_vec()))
}

impl Net {
    fn is_host(&self) -> bool {
        self.sess.as_ref().is_some_and(|s| s.host)
    }
    fn local(&self, id: u32) -> bool {
        self.players.get(&id).is_some_and(|p| p.node == self.my_node)
    }
    fn local_ids(&self) -> Vec<u32> {
        self.players.values().filter(|p| p.node == self.my_node).map(|p| p.id).collect()
    }
    fn sync_count(&mut self) {
        let n = self.players.len() as u32;
        if let Some(s) = &mut self.sess {
            s.desc.cur = n;
        }
    }
    fn deliver(&mut self, to: u32, from: u32, msg: Msg) {
        if let Some(q) = self.queues.get_mut(&to) {
            q.push_back((from, msg));
            if let Some(p) = self.players.get(&to) {
                if let Some(Obj::Event(s)) = handles::get(p.event) {
                    *s.state.lock().unwrap() = true;
                    s.cond.notify_all();
                }
            }
        }
    }
    /// A system message (from player 0) for every local player (or one).
    fn sys_to(&mut self, only: Option<u32>, data: Vec<u8>, relocs: Vec<usize>) {
        let ids = match only {
            Some(i) => vec![i],
            None => self.local_ids(),
        };
        for id in ids {
            self.deliver(id, 0, Msg { from: 0, data: data.clone(), relocs: relocs.clone() });
        }
    }
    fn sys_create(&mut self, only: Option<u32>, p: &Player) {
        let mut b = vec![0u8; 48];
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, DPSYS_CREATEPLAYERORGROUP);
        put(&mut b, 4, 1);
        put(&mut b, 8, p.id);
        put(&mut b, 12, self.players.len() as u32);
        let mut relocs = Vec::new();
        if !p.data.is_empty() {
            { let l = b.len() as u32; put(&mut b, 16, l); }
            relocs.push(16);
            b.extend_from_slice(&p.data);
        }
        put(&mut b, 20, p.data.len() as u32);
        put(&mut b, 24, 16);
        for (o, s) in [(32usize, &p.short), (36, &p.long)] {
            if !s.is_empty() {
                { let l = b.len() as u32; put(&mut b, o, l); }
                relocs.push(o);
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
        }
        put(&mut b, 44, p.flags | if p.node == self.my_node { DPENUMPLAYERS_LOCAL } else { 0 });
        self.sys_to(only, b, relocs);
    }
    fn sys_destroy(&mut self, p: &Player) {
        let mut b = vec![0u8; 52];
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, DPSYS_DESTROYPLAYERORGROUP);
        put(&mut b, 4, 1);
        put(&mut b, 8, p.id);
        put(&mut b, 28, 16);
        let mut relocs = Vec::new();
        for (o, s) in [(36usize, &p.short), (40, &p.long)] {
            if !s.is_empty() {
                { let l = b.len() as u32; put(&mut b, o, l); }
                relocs.push(o);
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
        }
        put(&mut b, 48, p.flags | if p.node == self.my_node { DPENUMPLAYERS_LOCAL } else { 0 });
        self.sys_to(None, b, relocs);
    }
    fn sys_data(&mut self, id: u32, data: &[u8]) {
        let mut b = Vec::new();
        for v in [DPSYS_SETPLAYERORGROUPDATA, 1, id, 20, data.len() as u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let mut relocs = Vec::new();
        if data.is_empty() {
            b[12..16].copy_from_slice(&0u32.to_le_bytes());
        } else {
            relocs.push(12);
            b.extend_from_slice(data);
        }
        self.sys_to(None, b, relocs);
    }
    fn sys_desc(&mut self) {
        let Some(s) = &self.sess else { return };
        let d = s.desc.clone();
        let mut b = vec![0u8; 4 + 80];
        b[0..4].copy_from_slice(&DPSYS_SETSESSIONDESC.to_le_bytes());
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 4, 80);
        put(&mut b, 8, d.flags);
        b[12..28].copy_from_slice(&d.instance);
        b[28..44].copy_from_slice(&d.app);
        put(&mut b, 44, d.max);
        put(&mut b, 48, d.cur);
        let mut relocs = vec![52, 56];
        { let l = b.len() as u32; put(&mut b, 52, l); }
        b.extend_from_slice(d.name.as_bytes());
        b.push(0);
        { let l = b.len() as u32; put(&mut b, 56, l); }
        b.extend_from_slice(d.pass.as_bytes());
        b.push(0);
        relocs.sort();
        self.sys_to(None, b, relocs);
    }
    fn to_peer(&mut self, node: u32, kind: u8, body: &[u8]) {
        if let Some(s) = self.peers.get_mut(&node) {
            send_frame(s, kind, body);
        }
    }
    fn to_peers_except(&mut self, except: u32, kind: u8, body: &[u8]) {
        for (node, s) in &mut self.peers {
            if *node != except {
                send_frame(s, kind, body);
            }
        }
    }
    /// The host relays (or the sender hands over) a message: local players get it now, the others through their node.
    fn route(&mut self, origin: u32, from: u32, to: u32, flags: u32, payload: &[u8]) {
        let targets: Vec<Player> = self.players.values().filter(|p| p.id != from && (to == 0 || p.id == to)).cloned().collect();
        for t in targets {
            if t.node == self.my_node {
                self.deliver(t.id, from, Msg { from, data: payload.to_vec(), relocs: vec![] });
            } else if t.node != origin {
                let mut w = W::default();
                w.u32(from).u32(t.id).u32(flags).bytes(payload);
                self.to_peer(t.node, 9, &w.0);
            }
        }
    }
    /// The host tells the others the new player count (and whatever else is in the description).
    fn broadcast_desc(&mut self) {
        let Some(s) = &self.sess else { return };
        let mut w = W::default();
        s.desc.enc(&mut w);
        self.to_peers_except(u32::MAX, 12, &w.0);
    }
    fn remove_player(&mut self, id: u32) -> Option<Player> {
        let p = self.players.remove(&id)?;
        self.queues.remove(&id);
        self.sync_count();
        self.sys_destroy(&p);
        Some(p)
    }
    /// Forget everything about the session (the players stay out: `Close` or a lost connection).
    fn end_session(&mut self) {
        self.epoch += 1;
        self.sess = None;
        self.peers.clear();
        self.players.clear();
        self.queues.clear();
        self.lost = false;
    }
}

// ---- threads ----

fn reuse_udp(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::fd::FromRawFd;
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let one: libc::c_int = 1;
        for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT, libc::SO_BROADCAST] {
            libc::setsockopt(fd, libc::SOL_SOCKET, opt, &one as *const _ as *const libc::c_void, 4);
        }
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as _;
        addr.sin_port = port.to_be();
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) < 0 {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(UdpSocket::from_raw_fd(fd))
    }
}

/// Answers session searches while this node hosts a session.
fn responder(epoch: u64, tcp_port: u16) {
    let Ok(sock) = reuse_udp(DISCOVERY_PORT) else { return };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    let mut buf = [0u8; 256];
    loop {
        let got = sock.recv_from(&mut buf);
        let desc = {
            let g = net();
            let n = g.as_ref().unwrap();
            if n.epoch != epoch {
                return;
            }
            n.sess.as_ref().map(|s| s.desc.clone())
        };
        let (Ok((len, from)), Some(desc)) = (got, desc) else { continue };
        if len >= 21 && &buf[..5] == b"S3DPQ" && (buf[5..21] == [0; 16] || buf[5..21] == desc.app || desc.app == [0; 16]) {
            let mut w = W::default();
            w.0.extend_from_slice(b"S3DPR");
            w.0.extend_from_slice(&tcp_port.to_le_bytes());
            desc.enc(&mut w);
            let _ = sock.send_to(&w.0, from);
        }
    }
}

fn accept_loop(epoch: u64, l: TcpListener) {
    let _ = l.set_nonblocking(true);
    loop {
        if net().as_ref().unwrap().epoch != epoch {
            return;
        }
        match l.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_nodelay(true);
                std::thread::spawn(move || host_reader(epoch, s));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// The host's side of one client connection.
fn host_reader(epoch: u64, mut s: TcpStream) {
    let Some((1, body)) = read_frame(&mut s) else { return };
    let mut r = R(&body, 0);
    let (inst, pass) = (r.bytes(), r.str());
    let node = {
        let mut g = net();
        let n = n!(g);
        if n.epoch != epoch {
            return;
        }
        let d = n.sess.as_ref().unwrap().desc.clone();
        let err = if inst != d.instance { Some(DPERR_NOCONNECTION) } else if !d.joinable() { Some(DPERR_CANTADDPLAYER) } else if d.flags & DPSESSION_PASSWORDREQUIRED != 0 && pass != d.pass { Some(DPERR_INVALIDPASSWORD) } else { None };
        if let Some(e) = err {
            let mut w = W::default();
            w.u32(e);
            send_frame(&mut s, 3, &w.0);
            return;
        }
        let node = n.next_node;
        n.next_node += 1;
        let mut w = W::default();
        w.u32(node);
        d.enc(&mut w);
        w.u32(n.players.len() as u32);
        for p in n.players.values() {
            p.enc(&mut w);
        }
        send_frame(&mut s, 2, &w.0);
        n.peers.insert(node, s.try_clone().unwrap());
        node
    };
    if trace() {
        eprintln!("dplay: node {node} joined");
    }
    while let Some((kind, body)) = read_frame(&mut s) {
        let mut g = net();
        let n = n!(g);
        if n.epoch != epoch {
            return;
        }
        let mut r = R(&body, 0);
        match kind {
            4 => {
                let (short, long, flags, data) = (r.str(), r.str(), r.u32(), r.bytes());
                let id = n.next_player;
                n.next_player += 1;
                let p = Player { id, node, short, long, flags, data, event: 0 };
                n.players.insert(id, p.clone());
                n.sync_count();
                let mut w = W::default();
                w.u32(id);
                n.to_peer(node, 5, &w.0);
                let mut w = W::default();
                p.enc(&mut w);
                n.to_peers_except(node, 6, &w.0);
                n.sys_create(None, &p);
                n.broadcast_desc();
            }
            7 => {
                let id = r.u32();
                if n.players.get(&id).is_some_and(|p| p.node == node) {
                    n.remove_player(id);
                    let mut w = W::default();
                    w.u32(id);
                    n.to_peers_except(node, 8, &w.0);
                    n.broadcast_desc();
                }
            }
            9 => {
                let (from, to, flags, data) = (r.u32(), r.u32(), r.u32(), r.bytes());
                n.route(node, from, to, flags, &data);
            }
            10 => {
                let (id, data) = (r.u32(), r.bytes());
                if let Some(p) = n.players.get_mut(&id) {
                    p.data = data.clone();
                    n.sys_data(id, &data);
                    let mut w = W::default();
                    w.u32(id).bytes(&data);
                    n.to_peers_except(node, 10, &w.0);
                }
            }
            _ => {}
        }
        CV.notify_all();
    }
    let mut g = net();
    let n = n!(g);
    if n.epoch != epoch {
        return;
    }
    n.peers.remove(&node);
    let gone: Vec<u32> = n.players.values().filter(|p| p.node == node).map(|p| p.id).collect();
    for id in gone {
        n.remove_player(id);
        let mut w = W::default();
        w.u32(id);
        n.to_peers_except(node, 8, &w.0);
    }
    n.broadcast_desc();
    CV.notify_all();
}

/// A client's side of its connection to the host.
fn client_reader(epoch: u64, mut s: TcpStream) {
    while let Some((kind, body)) = read_frame(&mut s) {
        let mut g = net();
        let n = n!(g);
        if n.epoch != epoch {
            return;
        }
        let mut r = R(&body, 0);
        match kind {
            5 => n.create_reply = Some(r.u32()),
            6 => {
                let p = Player::dec(&mut r);
                n.players.insert(p.id, p.clone());
                n.sync_count();
                n.sys_create(None, &p);
            }
            8 => {
                let id = r.u32();
                n.remove_player(id);
            }
            9 => {
                let (from, to, _flags, data) = (r.u32(), r.u32(), r.u32(), r.bytes());
                n.deliver(to, from, Msg { from, data, relocs: vec![] });
            }
            10 => {
                let (id, data) = (r.u32(), r.bytes());
                if let Some(p) = n.players.get_mut(&id) {
                    p.data = data.clone();
                    n.sys_data(id, &data);
                }
            }
            11 | 12 => {
                let d = Desc::dec(&mut r);
                if let Some(s) = &mut n.sess {
                    s.desc = d;
                }
                if kind == 11 {
                    n.sys_desc();
                }
            }
            _ => {}
        }
        CV.notify_all();
    }
    let mut g = net();
    let n = n!(g);
    if n.epoch != epoch {
        return;
    }
    // The host is gone: the remote players with it.
    let me = n.my_node;
    n.players.retain(|_, p| p.node == me);
    let ids: Vec<u32> = n.players.keys().copied().collect();
    n.queues.retain(|k, _| ids.contains(k));
    n.lost = true;
    n.peers.clear();
    n.sys_to(None, DPSYS_SESSIONLOST.to_le_bytes().to_vec(), vec![]);
    CV.notify_all();
}

/// Asks the local network for sessions for `wait` and updates the list of those found.
fn search(app: [u8; 16], wait: Duration) {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = sock.set_broadcast(true);
    let mut q = b"S3DPQ".to_vec();
    q.extend_from_slice(&app);
    let mut dests: Vec<String> = vec![format!("255.255.255.255:{DISCOVERY_PORT}"), format!("127.0.0.1:{DISCOVERY_PORT}")];
    dests.extend(broadcast_addresses().into_iter().map(|a| format!("{a}:{DISCOVERY_PORT}")));
    // A host typed in by the player (another subnet) is asked directly.
    let typed = net().as_ref().unwrap().host_ip.clone();
    if !typed.is_empty() {
        dests.push(if typed.contains(':') { typed } else { format!("{typed}:{DISCOVERY_PORT}") });
    }
    for dest in dests {
        let _ = sock.send_to(&q, dest);
    }
    let end = Instant::now() + wait;
    let mut buf = [0u8; 1024];
    let mut found: Vec<(Desc, SocketAddr)> = Vec::new();
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let _ = sock.set_read_timeout(Some(left));
        let Ok((len, from)) = sock.recv_from(&mut buf) else { break };
        if len > 7 && &buf[..5] == b"S3DPR" {
            let port = u16::from_le_bytes([buf[5], buf[6]]);
            let d = Desc::dec(&mut R(&buf[7..len], 0));
            if !found.iter().any(|(f, _)| f.instance == d.instance) {
                found.push((d, SocketAddr::new(from.ip(), port)));
            }
        }
    }
    net().as_mut().unwrap().found = found;
}

// ---- the interface ----

fn random_guid() -> [u8; 16] {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut x = (t as u64) ^ ((std::process::id() as u64) << 32) ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = [0u8; 16];
    for b in &mut out {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 24) as u8;
    }
    out
}

/// A DPNAME (16 bytes) with the names after it, in the guest heap; `free` them with the returned pointer.
fn guest_name(c: &mut Cpu, short: &str, long: &str) -> u32 {
    let p = heap::alloc(c, 16 + short.len() as u32 + long.len() as u32 + 2);
    let m = c.mem;
    m.w32(p, 16);
    m.w32(p + 4, 0);
    let s = p + 16;
    let l = s + short.len() as u32 + 1;
    m.write_bytes(s, short.as_bytes());
    m.w8(s + short.len() as u32, 0);
    m.write_bytes(l, long.as_bytes());
    m.w8(l + long.len() as u32, 0);
    m.w32(p + 8, s);
    m.w32(p + 12, l);
    p
}

fn dp_close(_: &mut Cpu, _: &[u32]) -> u32 {
    let mut g = net();
    let n = n!(g);
    if let Some(s) = &mut n.sess {
        // Closing the sockets lets the other side see the end.
        let host = s.host;
        for (_, st) in n.peers.iter() {
            let _ = st.shutdown(std::net::Shutdown::Both);
        }
        if trace() {
            eprintln!("dplay: Close (host {host})");
        }
    }
    n.end_session();
    0
}

fn wait_until<T>(mut g: MutexGuard<'static, Option<Net>>, d: Duration, mut f: impl FnMut(&mut Net) -> Option<T>) -> (MutexGuard<'static, Option<Net>>, Option<T>) {
    let end = Instant::now() + d;
    loop {
        if let Some(v) = f(n!(g)) {
            return (g, Some(v));
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return (g, None);
        }
        g = CV.wait_timeout(g, left).unwrap().0;
    }
}

fn dp_create_player(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID*, DPNAME*, event, data*, size, flags)
    let (short, long) = if a[2] != 0 {
        let s = |o: u32| if c.mem.r32(a[2] + o) == 0 { String::new() } else { crate::kernel32::cstr(c, c.mem.r32(a[2] + o)) };
        (s(8), s(12))
    } else {
        (String::new(), String::new())
    };
    let data = if a[4] != 0 && a[5] != 0 { c.mem.read_bytes(a[4], a[5] as usize) } else { Vec::new() };
    let mut g = net();
    if n!(g).sess.is_none() || n!(g).lost {
        return DPERR_NOCONNECTION;
    }
    let mut p = Player { id: 0, node: n!(g).my_node, short, long, flags: a[6], data, event: a[3] };
    if n!(g).is_host() {
        if !n!(g).sess.as_ref().unwrap().desc.joinable() {
            return DPERR_CANTCREATEPLAYER;
        }
        p.id = n!(g).next_player;
        n!(g).next_player += 1;
    } else {
        let mut w = W::default();
        w.str(&p.short).str(&p.long).u32(p.flags).bytes(&p.data);
        n!(g).create_reply = None;
        n!(g).to_peer(0, 4, &w.0);
        let (g2, id) = wait_until(g, Duration::from_secs(5), |n| if n.lost { Some(0) } else { n.create_reply.take() });
        g = g2;
        match id {
            Some(id) if id != 0 => p.id = id,
            _ => return DPERR_CANTCREATEPLAYER,
        }
    }
    let n = n!(g);
    n.players.insert(p.id, p.clone());
    n.queues.insert(p.id, VecDeque::new());
    n.sync_count();
    if n.is_host() {
        let mut w = W::default();
        p.enc(&mut w);
        n.to_peers_except(u32::MAX, 6, &w.0);
        n.broadcast_desc();
    }
    let others: Vec<Player> = n.players.values().filter(|o| o.node != n.my_node).cloned().collect();
    for o in &others {
        n.sys_create(Some(p.id), o);
    }
    if trace() {
        eprintln!("dplay: CreatePlayer '{}' -> {:#x} (node {})", p.short, p.id, p.node);
    }
    drop(g);
    c.mem.w32(a[1], p.id);
    0
}

fn dp_destroy_player(_: &mut Cpu, a: &[u32]) -> u32 {
    let mut g = net();
    let n = n!(g);
    if !n.local(a[1]) {
        return DPERR_INVALIDPLAYER;
    }
    n.remove_player(a[1]);
    if n.is_host() {
        let mut w = W::default();
        w.u32(a[1]);
        n.to_peers_except(u32::MAX, 8, &w.0);
        n.broadcast_desc();
    } else {
        let mut w = W::default();
        w.u32(a[1]);
        n.to_peer(0, 7, &w.0);
    }
    0
}

fn dp_enum_players(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, guid*, callback, context, flags)
    let flags = a[4];
    let list: Vec<(Player, bool)> = {
        let g = net();
        let n = g.as_ref().unwrap();
        n.players
            .values()
            .map(|p| (p.clone(), p.node == n.my_node))
            .filter(|(_, l)| (flags & DPENUMPLAYERS_LOCAL == 0 || *l) && (flags & DPENUMPLAYERS_REMOTE == 0 || !*l))
            .collect()
    };
    for (p, local) in list {
        let name = guest_name(c, &p.short, &p.long);
        let r = crate::shims::guest_call(c, a[2], &[p.id, 1, name, p.flags | if local { DPENUMPLAYERS_LOCAL } else { 0 }, a[3]]);
        heap::free(c, name);
        if r == 0 {
            break;
        }
    }
    0
}

fn dp_enum_sessions(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPSESSIONDESC2*, timeout, callback, context, flags)
    let flags = a[5];
    if flags & DPENUMSESSIONS_STOPASYNC != 0 {
        return 0;
    }
    let want = if a[1] != 0 { Desc::read_guest(c, a[1]) } else { Desc::default() };
    if flags & DPENUMSESSIONS_ASYNC != 0 {
        let app = want.app;
        std::thread::spawn(move || search(app, Duration::from_millis(1000)));
    } else {
        search(want.app, Duration::from_millis(if a[2] == 0 { 1000 } else { a[2] as u64 }));
    }
    let list: Vec<Desc> = net()
        .as_ref()
        .unwrap()
        .found
        .iter()
        .map(|(d, _)| d.clone())
        .filter(|d| want.app == [0; 16] || d.app == want.app)
        .filter(|d| flags & DPENUMSESSIONS_AVAILABLE == 0 || d.joinable())
        .filter(|d| d.flags & DPSESSION_PASSWORDREQUIRED == 0 || flags & DPENUMSESSIONS_PASSWORDREQUIRED != 0)
        .collect();
    let tm = heap::alloc(c, 4);
    c.mem.w32(tm, a[2]);
    for d in list {
        let buf = heap::alloc(c, d.size());
        d.write_guest(c, buf);
        let r = crate::shims::guest_call(c, a[3], &[buf, tm, 0, a[4]]);
        heap::free(c, buf);
        if r == 0 {
            heap::free(c, tm);
            return 0;
        }
    }
    crate::shims::guest_call(c, a[3], &[0, tm, 1, a[4]]);
    heap::free(c, tm);
    0
}

fn dp_enum_connections(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, application guid*, callback, context, flags): one service provider, TCP/IP
    let blob = heap::alloc(c, 36);
    c.mem.write_bytes(blob, &DPAID_SERVICE_PROVIDER);
    c.mem.w32(blob + 16, 16);
    c.mem.write_bytes(blob + 20, &TCPIP_SP);
    let sp = heap::alloc(c, 16);
    c.mem.write_bytes(sp, &TCPIP_SP);
    let name = guest_name(c, "Internet TCP/IP Connection For DirectPlay", "");
    crate::shims::guest_call(c, a[2], &[sp, blob, 36, name, 1, a[3]]);
    heap::free(c, name);
    heap::free(c, sp);
    heap::free(c, blob);
    0
}

fn dp_get_caps(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPCAPS*, flags)
    if a[1] == 0 {
        return E_INVALIDARG;
    }
    let host = net().as_ref().unwrap().is_host();
    let size = c.mem.r32(a[1]);
    let v = [size, 0x20 | host as u32, 65536, 0, 65536, 100_000, 1, 65536, 40, 1000];
    for (i, x) in v.iter().enumerate() {
        if 4 * (i as u32 + 1) <= size {
            c.mem.w32(a[1] + 4 * i as u32, *x);
        }
    }
    0
}

fn dp_get_message_count(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID, DWORD*)
    let g = net();
    let n = g.as_ref().unwrap();
    let count = if a[1] == 0 { n.queues.values().map(|q| q.len()).sum::<usize>() } else if let Some(q) = n.queues.get(&a[1]) { q.len() } else { return DPERR_INVALIDPLAYER };
    drop(g);
    c.mem.w32(a[2], count as u32);
    0
}

fn copy_out(c: &mut Cpu, data: &[u8], buf: u32, size: u32) -> u32 {
    let cap = if buf == 0 { 0 } else { c.mem.r32(size) as usize };
    c.mem.w32(size, data.len() as u32);
    if buf == 0 || cap < data.len() {
        return DPERR_BUFFERTOOSMALL;
    }
    c.mem.write_bytes(buf, data);
    0
}

fn dp_get_player_data(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID, data*, size*, flags)
    let data = match net().as_ref().unwrap().players.get(&a[1]) {
        Some(p) => p.data.clone(),
        None => return DPERR_INVALIDPLAYER,
    };
    copy_out(c, &data, a[2], a[3])
}

fn dp_set_player_data(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID, data*, size, flags)
    let data = if a[2] != 0 && a[3] != 0 { c.mem.read_bytes(a[2], a[3] as usize) } else { Vec::new() };
    let mut g = net();
    let n = n!(g);
    let Some(p) = n.players.get_mut(&a[1]) else { return DPERR_INVALIDPLAYER };
    p.data = data.clone();
    let mut w = W::default();
    w.u32(a[1]).bytes(&data);
    if n.is_host() {
        n.to_peers_except(u32::MAX, 10, &w.0);
    } else {
        n.to_peer(0, 10, &w.0);
    }
    0
}

fn dp_get_session_desc(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPSESSIONDESC2*, size*)
    let Some(d) = net().as_ref().unwrap().sess.as_ref().map(|s| s.desc.clone()) else { return DPERR_NOSESSIONS };
    let need = d.size();
    let cap = if a[1] == 0 { 0 } else { c.mem.r32(a[2]) };
    c.mem.w32(a[2], need);
    if cap < need {
        return DPERR_BUFFERTOOSMALL;
    }
    d.write_guest(c, a[1]);
    0
}

fn dp_set_session_desc(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPSESSIONDESC2*, flags)
    let nd = Desc::read_guest(c, a[1]);
    let mut g = net();
    let n = n!(g);
    let Some(s) = &mut n.sess else { return DPERR_NOSESSIONS };
    if !s.host {
        return DPERR_ACCESSDENIED;
    }
    s.desc.flags = nd.flags;
    s.desc.max = nd.max;
    s.desc.name = nd.name;
    s.desc.pass = nd.pass;
    s.desc.user = nd.user;
    let mut w = W::default();
    s.desc.enc(&mut w);
    n.to_peers_except(u32::MAX, 11, &w.0);
    0
}

fn dp_initialize(_: &mut Cpu, _: &[u32]) -> u32 {
    let mut g = net();
    if n!(g).inited {
        return DPERR_ALREADYINITIALIZED;
    }
    n!(g).inited = true;
    0
}

/// The service provider and (optionally) the host address of a connection address: chunks of (GUID, size, data).
fn dp_initialize_connection(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, address*, flags)
    const DPAID_INET: [u8; 16] = [0xa0, 0x4d, 0xa5, 0xc4, 0xaf, 0xe0, 0xcf, 0x11, 0x9c, 0x4e, 0x00, 0xa0, 0xc9, 0x05, 0x42, 0x5e];
    let mut ip = String::new();
    if a[1] != 0 {
        let mut p = a[1];
        for _ in 0..16 {
            let size = c.mem.r32(p + 16);
            if size > 4096 {
                break;
            }
            if c.mem.read_bytes(p, 16) == DPAID_INET {
                ip = String::from_utf8_lossy(&c.mem.read_bytes(p + 20, size as usize)).trim_end_matches('\0').trim().to_string();
            }
            if c.mem.read_bytes(p, 16) == DPAID_SERVICE_PROVIDER && c.mem.read_bytes(p + 20, 16) != TCPIP_SP {
                return DPERR_NOCONNECTION;
            }
            p += 20 + size;
            if c.mem.read_bytes(p, 16) == [0; 16] {
                break;
            }
        }
    }
    if trace() {
        eprintln!("dplay: InitializeConnection host '{ip}'");
    }
    let mut g = net();
    n!(g).inited = true;
    n!(g).host_ip = ip;
    0
}

fn dp_open(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPSESSIONDESC2*, flags)
    if a[1] == 0 {
        return E_INVALIDARG;
    }
    let mut d = Desc::read_guest(c, a[1]);
    if trace() {
        eprintln!("dplay: Open flags {:#x} session '{}' max {} flags {:#x}", a[2], d.name, d.max, d.flags);
    }
    if a[2] & DPOPEN_CREATE != 0 {
        let Ok(l) = TcpListener::bind("0.0.0.0:0") else { return DPERR_CANTCREATESESSION };
        let port = l.local_addr().map_or(0, |a| a.port());
        let mut g = net();
        let n = n!(g);
        n.end_session();
        if d.instance == [0; 16] {
            d.instance = random_guid();
        }
        d.cur = 0;
        let epoch = n.epoch;
        n.sess = Some(Sess { desc: d, host: true });
        n.my_node = 1;
        n.next_node = 2;
        n.next_player = 0x1000;
        drop(g);
        std::thread::spawn(move || accept_loop(epoch, l));
        std::thread::spawn(move || responder(epoch, port));
        return 0;
    }
    if a[2] & DPOPEN_JOIN == 0 {
        return dperr(120);
    }
    let find = |inst: &[u8; 16]| net().as_ref().unwrap().found.iter().find(|(f, _)| f.instance == *inst).cloned();
    let target = find(&d.instance).or_else(|| {
        search(d.app, Duration::from_millis(1000));
        find(&d.instance)
    });
    let Some((found, addr)) = target else { return DPERR_NOSESSIONS };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) else { return DPERR_NOCONNECTION };
    let _ = s.set_nodelay(true);
    let mut w = W::default();
    w.bytes(&found.instance).str(&d.pass);
    send_frame(&mut s, 1, &w.0);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let reply = read_frame(&mut s);
    let _ = s.set_read_timeout(None);
    match reply {
        Some((2, body)) => {
            let mut r = R(&body, 0);
            let node = r.u32();
            let desc = Desc::dec(&mut r);
            let count = r.u32();
            let players: Vec<Player> = (0..count).map(|_| Player::dec(&mut r)).collect();
            let mut g = net();
            let n = n!(g);
            n.end_session();
            let epoch = n.epoch;
            n.sess = Some(Sess { desc, host: false });
            n.my_node = node;
            n.players = players.into_iter().map(|p| (p.id, p)).collect();
            n.peers.insert(0, s.try_clone().unwrap());
            drop(g);
            std::thread::spawn(move || client_reader(epoch, s));
            0
        }
        Some((3, body)) => R(&body, 0).u32(),
        _ => DPERR_NOCONNECTION,
    }
}

fn dp_receive(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID *from, DPID *to, flags, data*, size*)
    let flags = a[3];
    let mut g = net();
    let n = n!(g);
    let want_to = (flags & DPRECEIVE_TOPLAYER != 0).then(|| c.mem.r32(a[2]));
    let want_from = (flags & DPRECEIVE_FROMPLAYER != 0).then(|| c.mem.r32(a[1]));
    let mut ids: Vec<u32> = n.queues.keys().copied().collect();
    ids.sort();
    let mut hit = None;
    for id in ids {
        if want_to.is_some_and(|t| t != id) {
            continue;
        }
        if let Some(i) = n.queues[&id].iter().position(|(_, m)| want_from.is_none_or(|f| f == m.from)) {
            hit = Some((id, i));
            break;
        }
    }
    let Some((id, i)) = hit else { return DPERR_NOMESSAGES };
    let (_, msg) = &n.queues[&id][i];
    let len = msg.data.len() as u32;
    let cap = if a[4] == 0 { 0 } else { c.mem.r32(a[5]) };
    c.mem.w32(a[5], len);
    if cap < len {
        return DPERR_BUFFERTOOSMALL;
    }
    let mut data = msg.data.clone();
    for &o in &msg.relocs {
        let off = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        if off != 0 {
            data[o..o + 4].copy_from_slice(&(a[4] + off).to_le_bytes());
        }
    }
    c.mem.write_bytes(a[4], &data);
    c.mem.w32(a[1], msg.from);
    c.mem.w32(a[2], id);
    if flags & DPRECEIVE_PEEK == 0 {
        n.queues.get_mut(&id).unwrap().remove(i);
    }
    0
}

fn dp_send(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, from, to, flags, data*, size)
    if a[5] > 65536 {
        return DPERR_SENDTOOBIG;
    }
    let data = if a[4] != 0 { c.mem.read_bytes(a[4], a[5] as usize) } else { Vec::new() };
    let mut g = net();
    let n = n!(g);
    if !n.local(a[1]) {
        return DPERR_INVALIDPLAYER;
    }
    if n.sess.is_none() || n.lost {
        return DPERR_NOCONNECTION;
    }
    if a[2] != 0 && !n.players.contains_key(&a[2]) {
        return DPERR_INVALIDPLAYER;
    }
    let me = n.my_node;
    n.route(me, a[1], a[2], a[3], &data);
    if !n.is_host() && (a[2] == 0 || !n.local(a[2])) {
        let mut w = W::default();
        w.u32(a[1]).u32(a[2]).u32(a[3]).bytes(&data);
        n.to_peer(0, 9, &w.0);
    }
    0
}

fn nop(_: &mut Cpu, _: &[u32]) -> u32 {
    E_NOTIMPL
}

const NAMES: &[&str] = &[
    "QueryInterface", "AddRef", "Release", "AddPlayerToGroup", "Close", "CreateGroup", "CreatePlayer", "DeletePlayerFromGroup", "DestroyGroup", "DestroyPlayer", "EnumGroupPlayers", "EnumGroups",
    "EnumPlayers", "EnumSessions", "GetCaps", "GetGroupData", "GetGroupName", "GetMessageCount", "GetPlayerAddress", "GetPlayerCaps", "GetPlayerData", "GetPlayerName", "GetSessionDesc", "Initialize",
    "Open", "Receive", "Send", "SetGroupData", "SetGroupName", "SetPlayerData", "SetPlayerName", "SetSessionDesc", "AddGroupToGroup", "CreateGroupInGroup", "DeleteGroupFromGroup", "EnumConnections",
    "EnumGroupsInGroup", "GetGroupConnectionSettings", "InitializeConnection", "SecureOpen", "SendChatMessage", "SetGroupConnectionSettings", "StartSession", "GetGroupFlags", "GetGroupParent",
    "GetPlayerAccount", "GetPlayerFlags", "GetGroupOwner", "SetGroupOwner", "SendEx", "GetMessageQueue", "CancelMessage", "CancelPriority",
];

/// A DirectPlay object (IDirectPlay4A; the older interfaces answer from the same table).
pub fn create(c: &mut Cpu) -> u32 {
    static VTABLE: Mutex<u32> = Mutex::new(0);
    let mut g = VTABLE.lock().unwrap();
    if *g == 0 {
        *g = make_vtable(
            c,
            "IDirectPlay4",
            NAMES,
            &[
                ("QueryInterface", 3, |c, a| {
                    c.mem.w32(a[2], a[0]);
                    add_ref(c, a[0]);
                    0
                }),
                ("AddRef", 1, |c, a| add_ref(c, a[0])),
                ("Release", 1, |c, a| release(c, a[0])),
                ("AddPlayerToGroup", 3, nop),
                ("Close", 1, dp_close),
                ("CreateGroup", 6, nop),
                ("CreatePlayer", 7, dp_create_player),
                ("DeletePlayerFromGroup", 3, nop),
                ("DestroyGroup", 2, nop),
                ("DestroyPlayer", 2, dp_destroy_player),
                ("EnumGroupPlayers", 6, nop),
                ("EnumGroups", 5, |c, a| {
                    // no groups: just the end of the list
                    let _ = (c, a);
                    0
                }),
                ("EnumPlayers", 5, dp_enum_players),
                ("EnumSessions", 6, dp_enum_sessions),
                ("GetCaps", 3, dp_get_caps),
                ("GetGroupData", 5, nop),
                ("GetGroupName", 4, nop),
                ("GetMessageCount", 3, dp_get_message_count),
                ("GetPlayerAddress", 4, nop),
                ("GetPlayerCaps", 4, dp_get_caps_player),
                ("GetPlayerData", 5, dp_get_player_data),
                ("GetPlayerName", 4, nop),
                ("GetSessionDesc", 3, dp_get_session_desc),
                ("Initialize", 2, dp_initialize),
                ("Open", 3, dp_open),
                ("Receive", 6, dp_receive),
                ("Send", 6, dp_send),
                ("SetGroupData", 5, nop),
                ("SetGroupName", 4, nop),
                ("SetPlayerData", 5, dp_set_player_data),
                ("SetPlayerName", 4, nop),
                ("SetSessionDesc", 3, dp_set_session_desc),
                ("AddGroupToGroup", 3, nop),
                ("CreateGroupInGroup", 6, nop),
                ("DeleteGroupFromGroup", 3, nop),
                ("EnumConnections", 5, dp_enum_connections),
                ("EnumGroupsInGroup", 6, nop),
                ("GetGroupConnectionSettings", 5, nop),
                ("InitializeConnection", 3, dp_initialize_connection),
                ("SecureOpen", 5, nop),
                ("SendChatMessage", 5, nop),
                ("SetGroupConnectionSettings", 4, nop),
                ("StartSession", 3, nop),
                ("GetGroupFlags", 3, nop),
                ("GetGroupParent", 3, nop),
                ("GetPlayerAccount", 5, nop),
                ("GetPlayerFlags", 3, nop),
                ("GetGroupOwner", 3, nop),
                ("SetGroupOwner", 3, nop),
                ("SendEx", 10, nop),
                ("GetMessageQueue", 6, nop),
                ("CancelMessage", 3, nop),
                ("CancelPriority", 4, nop),
            ],
        );
    }
    new_object(c, *g)
}

fn dp_get_caps_player(c: &mut Cpu, a: &[u32]) -> u32 {
    // (this, DPID, DPCAPS*, flags)
    dp_get_caps(c, &[a[0], a[2], a[3]])
}

/// IDirectPlayLobby2::CreateCompoundAddress(this, DPCOMPOUNDADDRESSELEMENT*, count, address*, size*): the elements
/// (data type GUID, size, pointer) one after the other as chunks (GUID, size, data).
pub fn create_compound_address(c: &mut Cpu, a: &[u32]) -> u32 {
    let mut out = Vec::new();
    for i in 0..a[2] {
        let e = a[1] + 24 * i;
        let size = c.mem.r32(e + 16);
        out.extend_from_slice(&c.mem.read_bytes(e, 16));
        out.extend_from_slice(&size.to_le_bytes());
        if size != 0 {
            out.extend_from_slice(&c.mem.read_bytes(c.mem.r32(e + 20), size as usize));
        }
    }
    copy_out(c, &out, a[3], a[4])
}

/// The broadcast addresses of the network interfaces that are up.
fn broadcast_addresses() -> Vec<std::net::Ipv4Addr> {
    let mut out = Vec::new();
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut p = list;
        while !p.is_null() {
            let i = &*p;
            let up = i.ifa_flags & (libc::IFF_UP as u32) != 0 && i.ifa_flags & (libc::IFF_BROADCAST as u32) != 0;
            #[cfg(target_os = "macos")]
            let broadcast = i.ifa_dstaddr;
            #[cfg(not(target_os = "macos"))]
            let broadcast = i.ifa_ifu;
            if up && !broadcast.is_null() && (*broadcast).sa_family as i32 == libc::AF_INET {
                let sin = &*(broadcast as *const libc::sockaddr_in);
                out.push(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
            }
            p = i.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    out
}
