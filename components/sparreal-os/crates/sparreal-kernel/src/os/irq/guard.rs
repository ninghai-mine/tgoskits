use crate::hal::al;

pub struct NoIrqGuard {
    is_enabled: bool,
}

impl NoIrqGuard {
    pub fn new() -> Self {
        let is_enabled = al::cpu::irq_local_is_enabled();
        al::cpu::irq_local_set_enable(false);
        Self { is_enabled }
    }
}

impl Default for NoIrqGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NoIrqGuard {
    fn drop(&mut self) {
        if self.is_enabled {
            al::cpu::irq_local_set_enable(true);
        }
    }
}
