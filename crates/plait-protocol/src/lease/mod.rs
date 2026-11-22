pub mod cleanup;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirIdentity {
    pub dev: u64,
    pub ino: u64,
}
