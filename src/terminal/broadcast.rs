//! Current-tab pane input broadcast (V1).
//!
//! Scope is deliberately narrow: fan-out keyboard/paste bytes to sibling panes
//! in the same tab. Password auto-fill stays per-pane — secrets are written
//! with a direct `terminal.write` that never emits [`BroadcastInput`].

use gpui::EntityId;

/// User keystrokes / paste that should be mirrored to broadcast receivers.
#[derive(Clone, Debug)]
pub struct BroadcastInput {
    pub bytes: Vec<u8>,
}

/// Whether a pane participates as a broadcast receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastRole {
    /// Broadcast is off for the tab.
    Off,
    /// Focused source of the broadcast (still shows the group indicator).
    Source,
    /// Receives mirrored input.
    Receiver,
    /// Explicitly opted out of the current tab's broadcast group.
    OptedOut,
}

/// Resolve which sibling panes should receive `source`'s input.
///
/// `members` is `(entity_id, opted_out)` for every ready terminal in the tab.
/// The source itself is never listed as a receiver. Connecting / pending slots
/// are not members.
pub fn broadcast_receivers(
    broadcast_enabled: bool,
    source: EntityId,
    members: &[(EntityId, bool)],
) -> Vec<EntityId> {
    if !broadcast_enabled {
        return Vec::new();
    }
    members
        .iter()
        .filter(|(id, opted_out)| *id != source && !*opted_out)
        .map(|(id, _)| *id)
        .collect()
}

/// Role of `pane` inside a tab's broadcast group.
pub fn broadcast_role(
    broadcast_enabled: bool,
    pane: EntityId,
    focused: Option<EntityId>,
    opted_out: bool,
) -> BroadcastRole {
    if !broadcast_enabled {
        return BroadcastRole::Off;
    }
    if opted_out {
        return BroadcastRole::OptedOut;
    }
    if focused == Some(pane) {
        BroadcastRole::Source
    } else {
        BroadcastRole::Receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId::from(n)
    }

    #[test]
    fn receivers_empty_when_broadcast_off() {
        let members = [(id(1), false), (id(2), false)];
        assert!(broadcast_receivers(false, id(1), &members).is_empty());
    }

    #[test]
    fn receivers_exclude_source_and_opt_outs() {
        let members = [(id(1), false), (id(2), true), (id(3), false)];
        assert_eq!(broadcast_receivers(true, id(1), &members), vec![id(3)]);
    }

    #[test]
    fn role_marks_source_receiver_and_opt_out() {
        assert_eq!(
            broadcast_role(true, id(1), Some(id(1)), false),
            BroadcastRole::Source
        );
        assert_eq!(
            broadcast_role(true, id(2), Some(id(1)), false),
            BroadcastRole::Receiver
        );
        assert_eq!(
            broadcast_role(true, id(2), Some(id(1)), true),
            BroadcastRole::OptedOut
        );
        assert_eq!(
            broadcast_role(false, id(1), Some(id(1)), false),
            BroadcastRole::Off
        );
    }
}
