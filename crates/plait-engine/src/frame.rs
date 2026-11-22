//! Stream I/O for bounded remote frames.
use plait_protocol::frame::Frame;
use std::io::{self, Read, Write};

pub trait FrameIo: Sized {
    fn write_to(&self, writer: &mut impl Write) -> io::Result<()>;
    fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>>;
}

impl FrameIo for Frame {
    fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.encode()?)
    }

    fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
        let mut header = [0u8; 9];
        loop {
            match reader.read(&mut header[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        reader.read_exact(&mut header[1..])?;
        let (stream, kind, length) = Self::parse_header(header)?;
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload)?;
        Ok(Some(Self {
            stream,
            kind,
            payload,
        }))
    }
}
