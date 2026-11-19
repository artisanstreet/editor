use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};

pub const IN_FLIGHT: usize = 64;
pub const PER_PEER: usize = 16;

#[derive(Clone)]
pub struct Handshakes(Arc<Mutex<Slots>>);

struct Slots {
    in_flight: usize,
    limit: usize,
    per_peer: usize,
    by_peer: HashMap<IpAddr, usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum Refused {
    #[error("{in_flight} TLS handshakes are already in flight")]
    Full { in_flight: usize },
    #[error("{peer} already has {in_flight} TLS handshakes in flight")]
    Peer { peer: IpAddr, in_flight: usize },
}

pub struct Slot {
    handshakes: Handshakes,
    peer: IpAddr,
}

fn network_of(peer: IpAddr) -> IpAddr {
    match peer.to_canonical() {
        IpAddr::V6(address) => IpAddr::V6(Ipv6Addr::from_bits(
            address.to_bits() & !u128::from(u64::MAX),
        )),
        v4 @ IpAddr::V4(_) => v4,
    }
}

impl Handshakes {
    #[must_use]
    pub fn new(limit: usize, per_peer: usize) -> Self {
        Self(Arc::new(Mutex::new(Slots {
            in_flight: 0,
            limit,
            per_peer,
            by_peer: HashMap::new(),
        })))
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn admit(&self, peer: IpAddr) -> Result<Slot, Refused> {
        let peer = network_of(peer);
        let mut slots = self.slots();
        let held = slots.by_peer.get(&peer).copied().unwrap_or(0);
        if held >= slots.per_peer {
            return Err(Refused::Peer {
                peer,
                in_flight: held,
            });
        }
        if slots.in_flight >= slots.limit {
            return Err(Refused::Full {
                in_flight: slots.in_flight,
            });
        }
        *slots.by_peer.entry(peer).or_insert(0) += 1;
        slots.in_flight += 1;
        Ok(Slot {
            handshakes: self.clone(),
            peer,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut slots = self.handshakes.slots();
        slots.in_flight -= 1;
        let held = slots
            .by_peer
            .get_mut(&self.peer)
            .expect("invariant: an admitted peer is counted until its slot is dropped");
        *held -= 1;
        if *held == 0 {
            slots.by_peer.remove(&self.peer);
        }
    }
}

#[cfg(test)]
#[path = "../tests/handshakes.rs"]
mod tests;
