use bitcode::{Decode, Encode};

#[derive(Debug, Encode, Decode, Default, Clone, Copy)]
pub struct WorldPos {
    pub x: i32,
    pub y: i32,
}

impl WorldPos {
    pub fn as_array(&self) -> [i64; 2] {
        [self.x as i64, self.y as i64]
    }
}

#[derive(Debug, Encode, Decode, Hash, PartialEq, Eq, Clone, Copy, Default)]
pub struct PeerId(pub u64);

#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum Destination<PeerType> {
    Peers(Vec<PeerType>),
    Peer(PeerType),
    Host,
    Broadcast,
}

impl<T> Destination<T> {
    pub fn convert<A>(self) -> Destination<A>
    where
        A: From<T>,
        T: Copy,
        T: Clone,
    {
        match self {
            Destination::Peer(p) => Destination::Peer(p.into()),
            Destination::Peers(p) => {
                Destination::Peers(p.iter().cloned().map(|p| p.into()).collect())
            }
            Destination::Host => Destination::Host,
            Destination::Broadcast => Destination::Broadcast,
        }
    }
}
