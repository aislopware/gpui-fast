//! The test platform's natives: hosts that record what the platform did to
//! them, so tests can count platform calls without a window server.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    rc::{Rc, Weak},
};

use anyhow::Result;

use super::{
    NativeFrame, NativeFramePlacement, NativeHostParams, NativeId, NativePresent,
    PlatformNativeHost,
};

/// A test window's natives and what was done to them.
#[derive(Default)]
pub(crate) struct TestComposition {
    hosts: HashMap<NativeId, Weak<TestNativeHost>>,
    presented: NativeFrame,
    /// Presents that had to change something on the platform.
    pub(crate) transactional_presents: usize,
    /// Presents that changed nothing.
    pub(crate) plain_presents: usize,
    /// The keyboard target of the last present.
    pub(crate) keyboard: Option<NativeId>,
}

impl TestComposition {
    pub(crate) fn create_host(&mut self, params: NativeHostParams) -> Rc<TestNativeHost> {
        let host = Rc::new(TestNativeHost {
            id: params.id,
            focused: params.focused,
            placement: RefCell::default(),
            hidden: Cell::new(true),
            calls: Cell::new(0),
        });
        self.hosts.insert(params.id, Rc::downgrade(&host));
        host
    }

    /// Reconciles the hosts with a presented frame, as a platform does.
    pub(crate) fn present(&mut self, present: &NativePresent) {
        let changes = present.frame.changes_since(&self.presented);
        if changes.is_empty() {
            self.plain_presents += 1;
        } else {
            self.transactional_presents += 1;
        }
        for placement in &changes.placed {
            if let Some(host) = self.host(placement.id) {
                host.calls.set(host.calls.get() + 1);
                host.hidden.set(false);
                *host.placement.borrow_mut() = Some((*placement).clone());
            }
        }
        for id in &changes.hidden {
            if let Some(host) = self.host(*id) {
                host.calls.set(host.calls.get() + 1);
                host.hidden.set(true);
            }
        }
        self.keyboard = present.keyboard;
        self.presented = present.frame.clone();
    }

    fn host(&self, id: NativeId) -> Option<Rc<TestNativeHost>> {
        self.hosts.get(&id).and_then(Weak::upgrade)
    }
}

/// A native host on the test platform.
pub struct TestNativeHost {
    id: NativeId,
    focused: Rc<dyn Fn(bool)>,
    placement: RefCell<Option<NativeFramePlacement>>,
    hidden: Cell<bool>,
    calls: Cell<usize>,
}

impl TestNativeHost {
    /// The native this host is.
    pub fn id(&self) -> NativeId {
        self.id
    }

    /// Where the platform last placed the native.
    pub fn placement(&self) -> Option<NativeFramePlacement> {
        self.placement.borrow().clone()
    }

    /// Whether the platform hides the native.
    pub fn is_hidden(&self) -> bool {
        self.hidden.get()
    }

    /// How many times the platform changed the native's container.
    pub fn calls(&self) -> usize {
        self.calls.get()
    }

    /// Acts as the platform does when the native takes the keyboard focus
    /// (`true`) or the user moves it elsewhere (`false`).
    pub fn simulate_focus(&self, focused: bool) {
        (self.focused)(focused);
    }
}

impl PlatformNativeHost for TestNativeHost {
    unsafe fn attach_view(&self, _view: NonNull<c_void>) -> Result<()> {
        Ok(())
    }

    unsafe fn attach_layer(&self, _layer: NonNull<c_void>) -> Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
