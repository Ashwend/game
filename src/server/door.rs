//! Server authority for code-locked doors: hanging a door in a doorway,
//! the open/close interaction, and the code lock.
//!
//! Lock model: the door stores its code in plain text (it's a game lock,
//! not a vault) plus the list of account ids that have entered it. Nobody
//! is authorized at hang time, the placer set the code but still proves
//! it at the door once, exactly the flow the genre trained players on.
//! Changing the code revokes every authorization except the changer's.

use crate::{
    game_balance::{DOOR_CODE_MAX_LEN, DOOR_CODE_MIN_LEN, DOOR_MAX_HP},
    items::{DeployableKind, HEWN_LOG_DOOR_ID, item_definition},
    protocol::{
        AccountId, ClientId, DeployedEntityId, DoorCommand, ServerMessage, ToastKind, ToastMessage,
    },
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope, deployables::DeployedEntity,
    inventory::take_items_from_inventory,
};

use crate::game_balance::{
    DEPLOYABLE_DAMAGE_RANGE_M as INTERACT_RANGE_M,
    DEPLOYABLE_PLACEMENT_REACH_M as PLACEMENT_REACH_M,
};

/// Code-lock + hinge state for one door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoorState {
    /// The lock code, digits only, length-validated on every set.
    pub(crate) code: String,
    /// Accounts that have entered the current code at this door.
    pub(crate) authorized: Vec<AccountId>,
    pub(crate) open: bool,
    /// The doorway building block this door hangs in. Destroying the
    /// doorway destroys the door.
    pub(crate) parent: DeployedEntityId,
}

impl DoorState {
    pub(crate) fn from_persisted(p: crate::save::PersistedDoorState) -> Self {
        Self {
            code: p.code,
            authorized: p.authorized,
            open: p.open,
            parent: p.parent,
        }
    }

    pub(crate) fn to_persisted(&self) -> crate::save::PersistedDoorState {
        crate::save::PersistedDoorState {
            code: self.code.clone(),
            authorized: self.authorized.clone(),
            open: self.open,
            parent: self.parent,
        }
    }
}

/// A valid code is 4-6 ASCII digits. Anything else is rejected before it
/// touches door state.
fn code_is_valid(code: &str) -> bool {
    (DOOR_CODE_MIN_LEN..=DOOR_CODE_MAX_LEN).contains(&code.len())
        && code.bytes().all(|byte| byte.is_ascii_digit())
}

impl GameServer {
    pub(super) fn apply_door_command(
        &mut self,
        client_id: ClientId,
        command: DoorCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            DoorCommand::Place {
                doorway_id,
                flip,
                code,
            } => self.place_door(client_id, doorway_id, flip, code),
            DoorCommand::Interact { id } => self.interact_door(client_id, id),
            DoorCommand::EnterCode { id, code } => self.enter_door_code(client_id, id, code),
            DoorCommand::ChangeCode { id, code } => self.change_door_code(client_id, id, code),
        }
    }

    fn place_door(
        &mut self,
        client_id: ClientId,
        doorway_id: DeployedEntityId,
        flip: bool,
        code: String,
    ) -> Vec<ServerEnvelope> {
        if !code_is_valid(&code) {
            return door_toast(
                client_id,
                ToastKind::Warning,
                format!("Codes are {DOOR_CODE_MIN_LEN}-{DOOR_CODE_MAX_LEN} digits"),
            );
        }
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let owner = client.account_id;
        let feet = client.controller.position;
        let Some(doorway) = self.deployed_entities.get(&doorway_id) else {
            return door_toast(client_id, ToastKind::Warning, "No doorway there".to_owned());
        };
        if !matches!(
            doorway.kind,
            DeployableKind::Building {
                piece: crate::building::BuildingPiece::Doorway,
                ..
            }
        ) {
            return door_toast(
                client_id,
                ToastKind::Warning,
                "Doors only mount in doorways".to_owned(),
            );
        }
        if !feet.within_horizontal_range(doorway.position, PLACEMENT_REACH_M) {
            return door_toast(client_id, ToastKind::Warning, "Too far away".to_owned());
        }
        let occupied = self.deployed_entities.values().any(|entity| {
            entity
                .door
                .as_ref()
                .is_some_and(|door| door.parent == doorway_id)
        });
        if occupied {
            return door_toast(
                client_id,
                ToastKind::Warning,
                "That doorway already has a door".to_owned(),
            );
        }
        // The door inherits the doorway's pose; flipping mirrors hinge and
        // swing together by rotating half a turn.
        let position = doorway.position;
        let yaw = crate::building::snap_yaw_quarter_turn(
            doorway.yaw + if flip { std::f32::consts::PI } else { 0.0 },
        );

        let Some(definition) = item_definition(HEWN_LOG_DOOR_ID) else {
            return Vec::new();
        };
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if take_items_from_inventory(&mut client.inventory, HEWN_LOG_DOOR_ID, 1) != 1 {
            return door_toast(
                client_id,
                ToastKind::Warning,
                format!("You don't have a {}", definition.name),
            );
        }

        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id = self.next_deployed_entity_id.saturating_add(1);
        let entity = DeployedEntity {
            id,
            item_id: crate::items::intern_item_id(HEWN_LOG_DOOR_ID),
            kind: DeployableKind::Door,
            position,
            yaw,
            health: DOOR_MAX_HP,
            max_health: DOOR_MAX_HP,
            owner: Some(owner),
            furnace: None,
            placed_at_tick: self.tick,
            door: Some(DoorState {
                code,
                authorized: Vec::new(),
                open: false,
                parent: doorway_id,
            }),
            label: None,
            stability: 100,
            storage: None,
        };
        self.insert_deployed_entity(id, entity);
        self.chunk_manager.track_deployed_entity(id, position);
        // Doors inherit their doorway's stability; pull it in.
        self.refresh_structural_stability();

        door_toast(
            client_id,
            ToastKind::Success,
            "Door hung. Enter the code once to unlock it.".to_owned(),
        )
    }

    /// E-press on a door: authorized accounts toggle it; everyone else is
    /// prompted for the code.
    fn interact_door(&mut self, client_id: ClientId, id: DeployedEntityId) -> Vec<ServerEnvelope> {
        let Some(account) = self.door_actor_in_range(client_id, id) else {
            return Vec::new();
        };
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        let Some(door) = entity.door.as_mut() else {
            return Vec::new();
        };
        if door.authorized.contains(&account) {
            door.open = !door.open;
            // Open state replicates via `DeployableActive`; no toast, the
            // door swinging is its own feedback.
            Vec::new()
        } else {
            vec![ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::DoorCodePrompt { id },
            }]
        }
    }

    /// A correct code *authorizes* the account, it does not swing the
    /// door: entering the code is unlocking, opening stays an explicit
    /// E-press. Both outcomes also ship a `DoorCodeResult` so the keypad
    /// can play its accepted/denied sound.
    fn enter_door_code(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
        code: String,
    ) -> Vec<ServerEnvelope> {
        let Some(account) = self.door_actor_in_range(client_id, id) else {
            return Vec::new();
        };
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        let Some(door) = entity.door.as_mut() else {
            return Vec::new();
        };
        let code_result = |accepted: bool| ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::DoorCodeResult { accepted },
        };
        if door.code != code {
            let mut out = door_toast(client_id, ToastKind::Error, "Wrong code".to_owned());
            out.push(code_result(false));
            return out;
        }
        if !door.authorized.contains(&account) {
            door.authorized.push(account);
        }
        let mut out = door_toast(client_id, ToastKind::Success, "Unlocked".to_owned());
        out.push(code_result(true));
        out
    }

    /// Change the code: only an account that knows the current code (is
    /// authorized) may rotate it, and rotating revokes everyone else.
    fn change_door_code(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
        code: String,
    ) -> Vec<ServerEnvelope> {
        if !code_is_valid(&code) {
            return door_toast(
                client_id,
                ToastKind::Warning,
                format!("Codes are {DOOR_CODE_MIN_LEN}-{DOOR_CODE_MAX_LEN} digits"),
            );
        }
        let Some(account) = self.door_actor_in_range(client_id, id) else {
            return Vec::new();
        };
        let Some(entity) = self.deployed_entity_mut(id) else {
            return Vec::new();
        };
        let Some(door) = entity.door.as_mut() else {
            return Vec::new();
        };
        if !door.authorized.contains(&account) {
            return door_toast(
                client_id,
                ToastKind::Warning,
                "Enter the current code first".to_owned(),
            );
        }
        door.code = code;
        door.authorized = vec![account];
        door_toast(client_id, ToastKind::Success, "Code changed".to_owned())
    }

    /// Range + existence gate shared by every door interaction. Returns
    /// the actor's account id.
    fn door_actor_in_range(&self, client_id: ClientId, id: DeployedEntityId) -> Option<AccountId> {
        let client = self.clients.get(&client_id)?;
        let entity = self.deployed_entities.get(&id)?;
        if !matches!(entity.kind, DeployableKind::Door) {
            return None;
        }
        client
            .controller
            .position
            .within_horizontal_range(entity.position, INTERACT_RANGE_M)
            .then_some(client.account_id)
    }
}

fn door_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}
