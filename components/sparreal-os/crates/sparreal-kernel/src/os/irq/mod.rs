mod guard;

use alloc::{boxed::Box, collections::btree_map::BTreeMap};
pub use guard::*;

use crate::{hal::al::IrqId, os::sync::IrqSpinlock};

static IRQ_VEC: IrqSpinlock<BTreeMap<IrqId, Box<dyn Fn() + Send + Sync>>> =
    IrqSpinlock::new(BTreeMap::new());

pub fn register_handler<F>(irq: IrqId, handler: F)
where
    F: Fn() + Send + Sync + 'static,
{
    {
        let mut guard = IRQ_VEC.lock();
        guard.insert(irq, Box::new(handler));
    }
    crate::hal::al::platform::irq_set_enabled(irq, true);
}

pub(crate) fn handle_irq(irq: IrqId) {
    let guard = IRQ_VEC.lock();
    if let Some(handler) = guard.get(&irq) {
        handler();
    }
}
