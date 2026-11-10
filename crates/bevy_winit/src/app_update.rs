//! Main-thread App access immediately before a normal Winit-driven update.
//!
//! This hook does not create a schedule, event loop or render-quiescence barrier.
//! Hosts must coordinate background/render work before replacing native code.
use bevy_app::App;
use bevy_ecs::prelude::Resource;

/// One host callback receiving the real App outside its schedule execution.
///
/// Called after plugin cleanup and event forwarding, before each serviced update.
/// It may use normal App APIs permitted after plugin finalization, including
/// configuring schedules and resources. It must not add startup plugins, call
/// `App::update` recursively or replace the native runner/App. Panics propagate.
/// The callback may remove or replace this resource for subsequent updates.
#[derive(Resource, Clone, Copy)]
pub struct AppUpdateHook(pub fn(&mut App));

/// Host cleanup during Winit `exiting`, before native windows or the World clear.
/// Removed before invocation so normal explicit repeated cleanup is idempotent.
/// This does not join tasks or retire render work on behalf of the callback.
#[derive(Resource)]
pub struct AppCloseHook(pub fn(&mut App));

pub(crate) fn run(app: &mut App) {
    // End the resource borrow before granting exclusive access to the actual App.
    let callback = app
        .world()
        .get_resource::<AppUpdateHook>()
        .map(|hook| hook.0);
    if let Some(callback) = callback {
        callback(app);
    }
}

pub(crate) fn close(app: &mut App) {
    if let Some(hook) = app.world_mut().remove_resource::<AppCloseHook>() {
        (hook.0)(app);
    }
}

/// Service final App ownership while resources and native windows are still live.
/// Window disposal remains runner-owned and must occur while its loop is active.
pub(crate) fn exiting(app: &mut App, close_windows: impl FnOnce()) {
    close(app);
    close_windows();
    // Bevy 0.20 resources are entity-backed. No hook/resource lookup is legal
    // after this terminal clear; in particular not after run_app returns.
    app.world_mut().clear_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::Update;
    use bevy_ecs::prelude::*;
    use bevy_platform::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Resource, Default)]
    struct Calls {
        hook: usize,
        system: usize,
        close: usize,
    }

    fn service(app: &mut App) {
        let install = app.world().resource::<Calls>().hook == 0;
        app.world_mut().resource_mut::<Calls>().hook += 1;
        if install {
            app.add_systems(Update, |mut calls: ResMut<Calls>| {
                calls.system += 1;
            });
        }
    }

    #[test]
    fn callback_configures_the_real_app_before_its_update_and_can_be_removed() {
        let mut app = App::new();
        app.init_resource::<Calls>();
        app.finish();
        app.cleanup();
        app.insert_resource(AppUpdateHook(service));
        for expected in 1..=2 {
            run(&mut app);
            assert_eq!(app.world().resource::<Calls>().hook, expected);
            assert_eq!(app.world().resource::<Calls>().system, expected - 1);
            app.update();
            assert_eq!(app.world().resource::<Calls>().system, expected);
        }
        app.world_mut().remove_resource::<AppUpdateHook>();
        run(&mut app);
        assert_eq!(app.world().resource::<Calls>().hook, 2);
        app.update();
        assert_eq!(app.world().resource::<Calls>().system, 3);
        app.insert_resource(AppCloseHook(|app| {
            app.world_mut().resource_mut::<Calls>().close += 1;
        }));
        close(&mut app);
        close(&mut app);
        assert_eq!(app.world().resource::<Calls>().close, 1);
        assert_eq!(app.world().resource::<Calls>().system, 3);
    }

    #[derive(Default)]
    struct ExitOrder {
        sequence: AtomicUsize,
        calls: AtomicUsize,
        windows: AtomicUsize,
        world: AtomicUsize,
    }

    #[derive(Resource)]
    struct LiveOwner {
        value: usize,
        order: Arc<ExitOrder>,
    }
    impl Drop for LiveOwner {
        fn drop(&mut self) {
            self.order.world.store(
                self.order.sequence.fetch_add(1, Ordering::SeqCst),
                Ordering::SeqCst,
            );
        }
    }

    fn retire(app: &mut App) {
        assert!(!app.world().contains_resource::<AppCloseHook>());
        {
            let mut owner = app.world_mut().resource_mut::<LiveOwner>();
            assert_eq!(owner.value, 7);
            owner.value = 8;
            assert_eq!(owner.order.sequence.fetch_add(1, Ordering::SeqCst), 0);
            owner.order.calls.fetch_add(1, Ordering::SeqCst);
        }
        // Removing the callback before invocation also makes nested cleanup a
        // no-op while the real resource cache is still live.
        close(app);
    }

    #[test]
    fn exiting_retires_the_live_app_once_before_windows_and_entity_backed_resources() {
        let order = Arc::new(ExitOrder::default());
        let mut app = App::new();
        app.insert_resource(LiveOwner {
            value: 7,
            order: order.clone(),
        });
        app.finish();
        app.cleanup();
        app.insert_resource(AppCloseHook(retire));
        let windows = order.clone();
        exiting(&mut app, move || {
            windows.windows.store(
                windows.sequence.fetch_add(1, Ordering::SeqCst),
                Ordering::SeqCst,
            );
        });
        // Observe externally: resource access after clear_all is precisely the
        // invalid lifecycle operation this boundary must prevent.
        assert_eq!(order.calls.load(Ordering::SeqCst), 1);
        assert_eq!(order.windows.load(Ordering::SeqCst), 1);
        assert_eq!(order.world.load(Ordering::SeqCst), 2);
        assert_eq!(order.sequence.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn exiting_without_a_host_callback_still_disposes_windows_before_the_world() {
        let order = Arc::new(ExitOrder::default());
        let mut app = App::new();
        app.insert_resource(LiveOwner {
            value: 7,
            order: order.clone(),
        });
        let windows = order.clone();
        exiting(&mut app, move || {
            windows.windows.store(
                windows.sequence.fetch_add(1, Ordering::SeqCst),
                Ordering::SeqCst,
            );
        });
        assert_eq!(order.calls.load(Ordering::SeqCst), 0);
        assert_eq!(order.windows.load(Ordering::SeqCst), 0);
        assert_eq!(order.world.load(Ordering::SeqCst), 1);
        assert_eq!(order.sequence.load(Ordering::SeqCst), 2);
    }
}
