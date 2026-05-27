use bevy_egui::egui;

use crate::{
    analytics::{Analytics, Event},
    app::state::{ConfirmationAction, MenuState, SaveStore},
};

use super::{
    modal::{self, ConfirmationChoice},
    worlds::refresh_worlds,
};

pub(super) fn confirmation_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    store: &SaveStore,
    analytics: &Analytics,
) {
    let Some(dialog) = menu.confirmation.as_mut() else {
        return;
    };

    let output = modal::confirmation_modal(
        ctx,
        "confirmation_modal",
        &dialog.title,
        &dialog.body,
        &dialog.confirm_label,
        &dialog.cancel_label,
        !dialog.closing,
    );

    if let Some(choice) = output.choice {
        dialog.closing = true;
        dialog.confirmed = choice == ConfirmationChoice::Confirm;
        ctx.request_repaint();
    }

    if output.finished_closing {
        let Some(dialog) = menu.confirmation.take() else {
            return;
        };

        if dialog.confirmed {
            apply_confirmation_action(dialog.action, menu, store, analytics);
        }
    }
}

pub(super) fn notice_ui(ctx: &egui::Context, menu: &mut MenuState) {
    let Some(dialog) = menu.notice.as_mut() else {
        return;
    };

    let output = modal::notice_modal(
        ctx,
        "notice_modal",
        &dialog.title,
        &dialog.body,
        &dialog.confirm_label,
        !dialog.closing,
    );

    if output.choice.is_some() {
        dialog.closing = true;
        ctx.request_repaint();
    }

    if output.finished_closing {
        menu.notice = None;
    }
}

fn apply_confirmation_action(
    action: ConfirmationAction,
    menu: &mut MenuState,
    store: &SaveStore,
    analytics: &Analytics,
) {
    match action {
        ConfirmationAction::DeleteWorld { world_id } => match store.0.delete_world(world_id) {
            Ok(()) => {
                analytics.track(Event::WorldDeleted);
                refresh_worlds(menu, store);
            }
            Err(error) => menu.status = Some(format!("delete failed: {error}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::save::WorldStore;
    use uuid::Uuid;

    fn temp_store() -> SaveStore {
        SaveStore(WorldStore::new(
            std::env::temp_dir().join(format!("game-confirm-test-{}", Uuid::new_v4())),
        ))
    }

    #[test]
    fn delete_confirmation_action_refreshes_world_list() {
        let store = temp_store();
        let save = store
            .0
            .create_world("Delete Me", None)
            .expect("world should create");
        let mut menu = MenuState::default();

        refresh_worlds(&mut menu, &store);
        assert_eq!(menu.worlds.len(), 1);

        apply_confirmation_action(
            ConfirmationAction::DeleteWorld { world_id: save.id },
            &mut menu,
            &store,
            &Analytics::disabled(),
        );

        assert!(menu.worlds.is_empty());
        assert!(menu.status.is_none());

        let _ = fs::remove_dir_all(store.0.root());
    }

    #[test]
    fn delete_confirmation_reports_store_errors() {
        let bad_root = std::env::temp_dir().join(format!("game-confirm-file-{}", Uuid::new_v4()));
        fs::write(&bad_root, "not a directory").expect("file should write");
        let store = SaveStore(WorldStore::new(&bad_root));
        let mut menu = MenuState::default();

        apply_confirmation_action(
            ConfirmationAction::DeleteWorld {
                world_id: Uuid::new_v4(),
            },
            &mut menu,
            &store,
            &Analytics::disabled(),
        );

        assert!(
            menu.status
                .expect("status should exist")
                .contains("world list failed")
        );

        let _ = fs::remove_file(bad_root);
    }
}
