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

#[cfg(test)]
mod tests {
    use super::*;
    use plait_protocol::frame::Kind;

    #[test]
    fn stream_round_trip_and_truncated_input() {
        let frame = Frame {
            stream: 1,
            kind: Kind::Frame,
            payload: b"hello".to_vec(),
        };
        let mut bytes = Vec::new();
        frame.write_to(&mut bytes).unwrap();
        assert_eq!(
            Frame::read_from(&mut bytes.as_slice()).unwrap(),
            Some(frame)
        );
        assert_eq!(Frame::read_from(&mut &b""[..]).unwrap(), None);
        for end in 1..bytes.len() {
            assert_eq!(
                Frame::read_from(&mut &bytes[..end]).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }
}
