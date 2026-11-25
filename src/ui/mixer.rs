use super::Controller;
use crate::media::mixer::{Change, Mixer};
use slint::ComponentHandle;
use std::rc::Rc;

impl Controller {
    pub(super) fn stop_mixer(&self) {
        self.mixer.borrow_mut().take();
        self.volume_instance.take();
        self.flyout.set_adjusting_volume(false);
        self.flyout.set_volume_available(false);
        self.flyout.set_app_muted(false);
        self.flyout.set_app_volume(0);
    }

    pub(super) fn sync_mixer(self: &Rc<Self>) {
        if !self.flyout.window().is_visible() {
            return;
        }
        let snapshot = self.latest.borrow();
        let selected = snapshot
            .sessions
            .iter()
            .find(|s| Some(s.id) == snapshot.selected && !s.app_id.is_empty());
        if let Some(mixer) = self.mixer.borrow().as_ref()
            && selected.is_some_and(|s| s.id == mixer.selection.0 && s.app_id == mixer.selection.1)
        {
            return;
        }
        let was_adjusting = self.flyout.get_adjusting_volume();
        self.stop_mixer();
        if was_adjusting {
            self.arm_dismiss();
        }
        if let Some(selected) = selected {
            let weak = self.flyout.as_weak();
            *self.mixer.borrow_mut() =
                Mixer::start((selected.id, selected.app_id.clone()), move || {
                    let _ = weak.upgrade_in_event_loop(|flyout| flyout.invoke_volume_updated());
                })
                .ok();
        }
    }

    fn read_volume(self: &Rc<Self>) {
        let state = self.mixer.borrow().as_ref().and_then(Mixer::state);
        let instance = state.as_ref().map(|s| s.instance.clone());
        if self.volume_instance.replace(instance.clone()) != instance
            && self.flyout.get_adjusting_volume()
        {
            self.flyout.set_adjusting_volume(false);
            self.arm_dismiss();
        }
        self.flyout.set_volume_available(state.is_some());
        self.flyout
            .set_app_muted(state.as_ref().is_some_and(|s| s.muted));
        self.flyout.set_app_volume(state.map_or(0, |s| s.percent));
    }

    fn change_volume(self: &Rc<Self>, change: Change) {
        if let (Some(mixer), Some(instance)) =
            (&*self.mixer.borrow(), &*self.volume_instance.borrow())
        {
            mixer.change(instance.clone(), change);
        }
        self.arm_dismiss();
    }
}

pub(super) fn connect(controller: &Rc<Controller>) {
    let weak = Rc::downgrade(controller);
    controller.flyout.on_volume_updated(move || {
        if let Some(c) = weak.upgrade() {
            c.read_volume();
        }
    });
    let weak = Rc::downgrade(controller);
    controller.flyout.on_volume_interaction(move || {
        if let Some(c) = weak.upgrade() {
            c.arm_dismiss();
        }
    });
    let weak = Rc::downgrade(controller);
    controller.flyout.on_set_volume(move |percent| {
        if let Some(c) = weak.upgrade() {
            c.change_volume(Change::Set(percent));
        }
    });
    let weak = Rc::downgrade(controller);
    controller.flyout.on_step_volume(move |delta| {
        if let Some(c) = weak.upgrade() {
            c.change_volume(Change::Step(delta));
        }
    });
    let weak = Rc::downgrade(controller);
    controller.flyout.on_toggle_mute(move || {
        if let Some(c) = weak.upgrade() {
            c.change_volume(Change::ToggleMute);
        }
    });
}
