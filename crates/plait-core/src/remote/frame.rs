//! Stream I/O for bounded remote frames.
pub use plait_protocol::frame::{ERROR_LIMIT, Frame, Kind, REPLY_LIMIT, REQUEST_LIMIT};
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

    #[test]
    fn remote_round_trips_and_bounds() {
        for (kind, stream, limit) in [
            (Kind::Open, 1, REQUEST_LIMIT),
            (Kind::Frame, 2, REPLY_LIMIT),
            (Kind::Error, 3, ERROR_LIMIT),
            (Kind::Close, 4, 0),
            (Kind::Ping, 0, 0),
        ] {
            let frame = Frame {
                stream,
                kind,
                payload: vec![42; limit],
            };
            let mut bytes = Vec::new();
            frame.write_to(&mut bytes).unwrap();
            assert_eq!(
                Frame::read_from(&mut bytes.as_slice()).unwrap(),
                Some(frame)
            );
            let oversized = Frame {
                stream,
                kind,
                payload: vec![0; limit + 1],
            };
            assert!(oversized.write_to(&mut Vec::new()).is_err());
            let mut header = Vec::new();
            header.extend_from_slice(&stream.to_be_bytes());
            header.push(kind as u8);
            header.extend_from_slice(&((limit + 1) as u32).to_be_bytes());
            assert_eq!(
                Frame::read_from(&mut header.as_slice()).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn remote_rejects_invalid_and_truncated_frames() {
        assert!(Frame::read_from(&mut &[][..]).unwrap().is_none());
        let frame = Frame {
            stream: 1,
            kind: Kind::Open,
            payload: b"hello".to_vec(),
        };
        let mut bytes = Vec::new();
        frame.write_to(&mut bytes).unwrap();
        for end in 1..bytes.len() {
            assert!(Frame::read_from(&mut &bytes[..end]).is_err());
        }
        bytes[4] = 99;
        assert!(Frame::read_from(&mut bytes.as_slice()).is_err());
        assert!(
            Frame {
                stream: 0,
                kind: Kind::Close,
                payload: vec![]
            }
            .write_to(&mut Vec::new())
            .is_err()
        );
        assert!(
            Frame {
                stream: 1,
                kind: Kind::Ping,
                payload: vec![]
            }
            .write_to(&mut Vec::new())
            .is_err()
        );
    }
}
