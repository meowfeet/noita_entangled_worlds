use std::{
    io::{self, BufReader, BufWriter, Read, Write},
    net::{Shutdown, TcpStream},
    sync::mpsc::{self, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

use tracing::info;

use super::{NoitaInbound, NoitaOutbound};

/// Upper bound on a single framed message, to stop a corrupt length prefix from turning into
/// a multi-gigabyte allocation. The largest legitimate message is a world update (~82 KB) plus
/// entity-sync batches, so this leaves a very wide margin.
const MAX_MESSAGE_LEN: usize = 64 * 1024 * 1024;

fn read_one(mut buf: impl Read) -> io::Result<NoitaOutbound> {
    let mut len_buf = [0u8; 4];
    buf.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_LEN {
        return Err(io::Error::other(format!(
            "Message length {len} exceeds the maximum of {MAX_MESSAGE_LEN}"
        )));
    }
    let mut out_buf = vec![0; len];
    buf.read_exact(&mut out_buf)?;
    bitcode::decode(&out_buf).map_err(io::Error::other)
}

/// The connection to the mod: length-prefixed bitcode messages over local TCP.
pub struct MessageSocket {
    socket: BufWriter<TcpStream>,
    recv_messages: mpsc::Receiver<io::Result<NoitaOutbound>>,
    reader_thread: Option<JoinHandle<()>>,
}

impl MessageSocket {
    pub fn new(socket: TcpStream) -> io::Result<Self> {
        socket.set_write_timeout(Some(Duration::from_secs(10)))?;
        // Nagle would hold back the small messages on the frame-critical path.
        socket.set_nodelay(true)?;
        let (sender, recv_messages) = mpsc::channel();
        let mut reader = BufReader::new(socket.try_clone()?);
        let reader_thread = Some(thread::spawn(move || {
            loop {
                let res = read_one(&mut reader);
                let failed = res.is_err();
                if sender.send(res).is_err() || failed {
                    break;
                }
            }
        }));

        Ok(Self {
            // A single world-sync chunk can reach ~82 KB, so size the buffer past that
            // instead of the default 8 KB, which sent every such message straight to the kernel.
            socket: BufWriter::with_capacity(256 * 1024, socket),
            recv_messages,
            reader_thread,
        })
    }

    pub fn try_read(&self) -> io::Result<Option<NoitaOutbound>> {
        match self.recv_messages.try_recv() {
            Ok(msg) => msg.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(io::Error::other("Reader thread stopped")),
        }
    }

    pub fn write(&mut self, value: &NoitaInbound) -> io::Result<()> {
        let encoded = bitcode::encode(value);
        let len = u32::try_from(encoded.len()).map_err(io::Error::other)?;
        self.socket.write_all(&len.to_le_bytes())?;
        self.socket.write_all(&encoded)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

impl Drop for MessageSocket {
    fn drop(&mut self) {
        self.socket.get_mut().shutdown(Shutdown::Both).ok();
        if let Some(handle) = self.reader_thread.take() {
            handle.join().ok();
        }
        info!("Message socket dropped");
    }
}
