//! Bounded, I/O-free remote frame encoding.
use std::io;

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
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        validate(self.stream, self.kind, self.payload.len())?;
        let mut bytes = Vec::with_capacity(9 + self.payload.len());
        bytes.extend_from_slice(&self.stream.to_be_bytes());
        bytes.push(self.kind as u8);
        bytes.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    pub fn parse_header(header: [u8; 9]) -> io::Result<(u32, Kind, usize)> {
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
        Ok((stream, kind, length))
    }

    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let header: [u8; 9] = bytes
            .get(..9)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?;
        let (stream, kind, length) = Self::parse_header(header)?;
        let payload = bytes
            .get(9..)
            .filter(|payload| payload.len() == length)
            .ok_or_else(invalid)?;
        Ok(Self {
            stream,
            kind,
            payload: payload.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_complete_valid_frames() {
        let frame = Frame {
            stream: 1,
            kind: Kind::Open,
            payload: b"hello".to_vec(),
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(Frame::parse(&bytes).unwrap(), frame);
        for end in 0..bytes.len() {
            assert!(Frame::parse(&bytes[..end]).is_err());
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(Frame::parse(&extra).is_err());
    }
}
