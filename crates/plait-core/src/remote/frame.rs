//! Bounded framing for a multiplexed remote link.
use std::io::{self, Read, Write};

pub const REQUEST_LIMIT: usize = 64 * 1024;
pub const REPLY_LIMIT: usize = 1024 * 1024;
pub const ERROR_LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Open = 1,
    Frame = 2,
    Close = 3,
    Error = 4,
    Ping = 5,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub stream: u32,
    pub kind: Kind,
    pub payload: Vec<u8>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid remote frame")
}

fn validate(stream: u32, kind: Kind, length: usize) -> io::Result<()> {
    let limit = match kind {
        Kind::Open => REQUEST_LIMIT,
        Kind::Frame => REPLY_LIMIT,
        Kind::Error => ERROR_LIMIT,
        Kind::Close | Kind::Ping => 0,
    };
    if length > limit || (stream == 0) != (kind == Kind::Ping) {
        return Err(invalid());
    }
    Ok(())
}

impl Frame {
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        validate(self.stream, self.kind, self.payload.len())?;
        writer.write_all(&self.stream.to_be_bytes())?;
        writer.write_all(&[self.kind as u8])?;
        writer.write_all(&(self.payload.len() as u32).to_be_bytes())?;
        writer.write_all(&self.payload)
    }

    /// Returns `None` only for EOF at a frame boundary.
    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
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
        let stream = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = match header[4] {
            1 => Kind::Open,
            2 => Kind::Frame,
            3 => Kind::Close,
            4 => Kind::Error,
            5 => Kind::Ping,
            _ => return Err(invalid()),
        };
        let length = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
        validate(stream, kind, length)?;
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
