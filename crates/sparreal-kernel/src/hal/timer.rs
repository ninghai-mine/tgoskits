pub(crate) fn init() {
    let timer_irq = crate::hal::al::cpu::timer_irq();
    crate::os::irq::register_handler(timer_irq, timer_irq_handler);
}

fn timer_irq_handler() {
    
}
