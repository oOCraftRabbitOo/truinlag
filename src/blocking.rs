/// # The blocking API to use with the truinlag engine
///
/// **Only use this API if you only ever send single commands. For proper communication, required
/// by e.g. the discord bot, the async API must be used in order to allow for two-way
/// communication** (or perhaps this api can be used with threads, idk)
use crate::commands;
use crate::error::Result;
use bincode;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    pub fn connect(addr: String) -> Result<Connection> {
        UnixStream::connect(addr)
            .map_err(|err| err.into())
            .map(|stream| Connection { stream })
    }

    pub fn send(&mut self, command: commands::Command) -> Result<()> {
        let serialized =
            bincode::serialize(&command).expect("should always be able to serialize commands");
        self.stream.write(&serialized)?;
        self.stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    }

    pub fn recv(&mut self) -> Result<commands::Response> {
        let mut buf: [u8; 1024] = [0; 1024];
        let bytes_read = self.stream.read(&mut buf)?;
        Ok(bincode::deserialize_from(&buf[0..bytes_read])?)
    }

    pub fn shutdown(self) -> Result<()> {
        self.stream.shutdown(std::net::Shutdown::Both)?;
        Ok(())
    }
}
