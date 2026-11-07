use std::mem;

use crate::protocol::PeerId;
use crate::protocol::des::{
    DesToProxy, FullEntityData, Gid, ProxyToDes, REQUEST_AUTHORITY_RADIUS, UpdateOrUpload,
    UpdatePosition,
};
use bitcode::{Decode, Encode};
use rstar::{RTree, primitives::GeomWithData};
use rustc_hash::FxHashMap;
use tracing::{info, warn};

use crate::save_state::{SaveState, SaveStateEntry};

#[derive(Encode, Decode, Default)]
pub struct EntityStorage {
    entities: FxHashMap<Gid, FullEntityData>,
}

impl SaveStateEntry for EntityStorage {
    const FILENAME: &'static str = "des_entity_storage";
}

/// Keeps track of the synced entities and who has authority over them.
pub struct DesManager {
    /// Saved by the host.
    pub entity_storage: EntityStorage,
    /// Entities without an authority, by position.
    rtree: RTree<GeomWithData<[i64; 2], Gid>>,
    authority: FxHashMap<Gid, PeerId>,
    pending_messages: Vec<(PeerId, ProxyToDes)>,
}

impl DesManager {
    pub fn new(save_state: &SaveState) -> Self {
        let entity_storage: EntityStorage = save_state.load().unwrap_or_default();
        let elements: Vec<_> = entity_storage
            .entities
            .iter()
            .map(|(&gid, ent)| GeomWithData::new(ent.pos.as_array(), gid))
            .collect();
        info!("Loaded {} synced entities", elements.len());
        Self {
            entity_storage,
            rtree: RTree::bulk_load(elements),
            authority: Default::default(),
            pending_messages: Vec::new(),
        }
    }

    fn remove_gid_from_tree(&mut self, gid: Gid) {
        if let Some(entity) = self.entity_storage.entities.get(&gid) {
            self.rtree
                .remove(&GeomWithData::new(entity.pos.as_array(), gid));
        }
    }

    fn add_gid_to_tree(&mut self, gid: Gid) {
        if let Some(entity) = self.entity_storage.entities.get(&gid) {
            let t = GeomWithData::new(entity.pos.as_array(), gid);
            self.rtree.remove(&t); // Makes sure there isn't a way to add the same point twice somehow.
            self.rtree.insert(t);
        }
    }

    fn handle_update(&mut self, update: UpdateOrUpload, source: PeerId) {
        match update {
            UpdateOrUpload::Upload(full_entity_data) => {
                self.authority.insert(full_entity_data.gid, source);
                self.entity_storage
                    .entities
                    .insert(full_entity_data.gid, full_entity_data);
            }
            UpdateOrUpload::Update(update) => {
                let UpdatePosition {
                    gid,
                    pos,
                    counter,
                    is_charmed,
                    hp,
                    max_hp,
                    phys,
                    synced_var,
                } = update;
                self.remove_gid_from_tree(gid);
                if let Some(entity) = self.entity_storage.entities.get_mut(&gid) {
                    entity.pos = pos;
                    entity.is_charmed = is_charmed;
                    entity.hp = hp;
                    entity.max_hp = max_hp;
                    entity.counter = counter;
                    entity.phys = phys;
                    entity.synced_var = synced_var;
                } else {
                    warn!("Failed to find entity {gid:?} to update");
                }
            }
        }
    }

    pub fn handle_noita_msg(&mut self, source: PeerId, msg: DesToProxy) {
        // TODO maybe check that authorities are correct?
        match msg {
            DesToProxy::UpdateWand(gid, wand) => {
                self.entity_storage
                    .entities
                    .entry(gid)
                    .and_modify(|data| data.wand = wand);
            }
            DesToProxy::DeleteEntity(gid, ent) => {
                if self.entity_storage.entities.contains_key(&gid) {
                    self.authority.remove(&gid);
                    self.entity_storage.entities.remove(&gid);
                    self.remove_gid_from_tree(gid);
                } else if let Some(ent) = ent {
                    self.pending_messages
                        .push((source, ProxyToDes::DeleteEntity(ent)));
                }
            }
            DesToProxy::ReleaseAuthority(gid) => {
                self.authority.remove(&gid);
                self.add_gid_to_tree(gid);
            }
            DesToProxy::RequestAuthority { pos } => {
                // drain_within_distance panics without this check. Funny.
                if self.rtree.size() == 0 {
                    return;
                }
                let radius = REQUEST_AUTHORITY_RADIUS;
                let mut auths = Vec::new();
                for point in self
                    .rtree
                    .drain_within_distance(pos.as_array(), i64::from(radius).pow(2))
                {
                    let gid = point.data;
                    self.authority.insert(gid, source);
                    if let Some(entity) = self.entity_storage.entities.get(&gid).cloned() {
                        auths.push(entity)
                    } else {
                        warn!("Expected to find entity data to give authority");
                    }
                }
                if !auths.is_empty() {
                    if auths.len() == 1 {
                        self.pending_messages
                            .push((source, ProxyToDes::GotAuthority(auths.pop().unwrap())));
                    } else {
                        self.pending_messages
                            .push((source, ProxyToDes::GotAuthoritys(auths)))
                    }
                }
            }
            DesToProxy::UpdatePosition(update) => self.handle_update(update, source),
            DesToProxy::UpdatePositions(updates) => {
                for update in updates {
                    self.handle_update(update, source)
                }
            }
            DesToProxy::TransferAuthorityTo(gid, peer_id) => {
                if let Some(entity) = self.entity_storage.entities.get(&gid).cloned() {
                    self.authority.insert(gid, peer_id);
                    self.pending_messages
                        .push((peer_id, ProxyToDes::GotAuthority(entity)));
                } else {
                    warn!("Failed to find entity {gid:?} to transfer authority");
                }
            }
        }
    }

    pub fn noita_disconnected(&mut self, source: PeerId) {
        // TODO also remove entities from affected clients faster.
        info!("Peer {source} disconnected, freeing entities that were under authority");
        let mut free_again = Vec::new();
        self.authority.retain(|gid, authority| {
            let remove = source == *authority;
            if remove {
                free_again.push(*gid);
            }
            !remove
        });
        for gid in free_again {
            self.add_gid_to_tree(gid);
        }
    }

    pub fn pending_messages(&mut self) -> Vec<(PeerId, ProxyToDes)> {
        mem::take(&mut self.pending_messages)
    }

    pub fn reset(&mut self) {
        self.entity_storage = Default::default();
        self.rtree = RTree::default();
        self.authority.clear();
        self.pending_messages.clear();
    }
}
