use std::{
    io::{Error, Read, Write},
    net::{self, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    str,
    sync::atomic::{AtomicU16, Ordering},
    thread,
};

use mirrored_buffer::MirroredBuffer;

struct Frame {}

struct Server<'a> {
    buf: MirroredBuffer<'a>,
    ln: TcpListener,
    pub local_addr: SocketAddr,
}

impl<'a> Server<'a> {
    fn new(ip: &str) -> Result<(Server<'a>, u16), Error> {
        let ln = TcpListener::bind(format!("{ip}:0"))?;
        let local_addr = ln.local_addr().unwrap();
        println!("server bound to {local_addr}");

        let buf = MirroredBuffer::new(4096, Some("server"), Some(0))
            .expect("could not initialize mirrored buffer");

        Ok((
            Server {
                buf,
                ln,
                local_addr,
            },
            local_addr.port(),
        ))
    }

    fn run(&mut self) -> Result<(), Error> {
        println!("server running, listening for connections");

        let (mut conn, peer_addr) = self.ln.accept()?;
        println!("server {} connected to {}", self.local_addr, peer_addr);

        // server writes a small frame followed by a big one that's partial.
        // a second write sends the rest of the partial frame
        // a mirrored buffer will do no memcpy to fit the remaining chunk
        // while a regular ring buffer will memcpy to have the message continous

        // max message size: 4096
        // header size: u16
        // max payload size: 4096 - u16 = 4080 bytes
        // first frame: u16 + small_frame_msg_size
        // second frame chunk1: u16 + (4096 - 2 * u16 - (small_frame_msg_size - n))
        // second frame chunk2: n
        // n varies: expect worse latencies for smaller n
        // small_frame_msg_size varies: expect worse latencies the smaller it is
        // client busy waits for message and we calculate how long does it take to
        //

        Ok(())
    }
}

struct Client<'a> {
    buf: MirroredBuffer<'a>,
    conn: TcpStream,
    pub local_addr: SocketAddr,
}

impl<'a> Client<'a> {
    fn new(peer_addr: &str) -> Result<Client<'a>, Error> {
        let conn = TcpStream::connect(peer_addr)?;
        let local_addr = conn.local_addr().unwrap();
        let peer_addr = conn.peer_addr().unwrap();

        println!("client {local_addr} connected to server {peer_addr}");

        let buf = MirroredBuffer::new(4096, Some("client"), Some(0))
            .expect("could not initialize mirrored buffer");

        Ok(Client {
            buf,
            conn,
            local_addr,
        })
    }

    fn run(&mut self) -> Result<(), Error> {
        println!("client running");
        Ok(())
    }
}

fn main() {
    let (mut server, server_port) = Server::new("127.0.0.1").unwrap();
    let server_thread = thread::spawn(move || {
        if let Err(err) = server.run() {
            panic!("server error {err}");
        }
    });

    let client_thread = thread::spawn(move || {
        let mut client = Client::new(format!("127.0.0.1:{}", server_port).as_str()).unwrap();
        if let Err(err) = client.run() {
            panic!("client error {err}");
        }
    });

    server_thread.join().unwrap();
    client_thread.join().unwrap();
}
