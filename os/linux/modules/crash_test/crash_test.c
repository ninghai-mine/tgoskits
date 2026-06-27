// crash_test.c — 内核崩溃测试模块
// 加载时根据 action 参数触发不同类型的崩溃
#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/slab.h>
#include <linux/compiler.h>
#include <linux/preempt.h>
#include <asm/processor.h>

static char *action = "null";
module_param(action, charp, 0);
MODULE_PARM_DESC(action, "null | write_null | bug | undefined | double_free | corrupt_mem | stack_overflow | exec_null | hang | double_fault");

static int __init crash_init(void)
{
    pr_info("crash_test: triggering action=%s\n", action);

    if (!strcmp(action, "null")) {
        pr_info("crash_test: NULL pointer dereference (READ)\n");
        *(volatile int *)0 = 0;           // Data Abort (read) → OOPS → Panic

    } else if (!strcmp(action, "write_null")) {
        pr_info("crash_test: NULL pointer dereference (WRITE)\n");
        *(volatile int *)0 = 1;           // Data Abort (write) → OOPS → Panic

    } else if (!strcmp(action, "bug")) {
        pr_info("crash_test: calling BUG()\n");
        BUG();                             // BRK → OOPS → Panic

    } else if (!strcmp(action, "undefined")) {
        pr_info("crash_test: undefined instruction\n");
        asm volatile(".inst 0x00000000");  // UDF → OOPS → Panic

    } else if (!strcmp(action, "double_free")) {
        void *p = kmalloc(64, GFP_KERNEL);
        pr_info("crash_test: double kfree (slab debug)\n");
        kfree(p);
        kfree(p);                          // Double-free → slab warning
        BUG();                             // force panic after corruption
    } else if (!strcmp(action, "corrupt_mem")) {
        // Wild write to corrupt random kernel memory — unpredictable crash
        pr_info("crash_test: corrupting random memory\n");
        volatile u64 *addr = (volatile u64 *)0xffff800081000000;
        *addr = 0xdeadbeef;               // wild write → unpredictable panic

    } else if (!strcmp(action, "stack_overflow")) {
        pr_info("crash_test: triggering stack overflow\n");
        // Recursively call ourselves to exhaust kernel stack
        crash_init();                      // Stack overflow → panic

    } else if (!strcmp(action, "exec_null")) {
        pr_info("crash_test: jumping to NULL (instruction fetch)\n");
        void (*fn)(void) = NULL;
        fn();                              // Instruction Abort from NULL

    } else if (!strcmp(action, "hang")) {
        // Kernel hang / deadlock — watchdog should detect after 60s.
        // Spin with preempt disabled so the CPU can't schedule away.
        pr_info("crash_test: entering infinite loop (hang test)\n");
        preempt_disable();
        while (1) {
            cpu_relax();                   // infinite busy-loop, watchdog timeout
        }
        preempt_enable();

    } else if (!strcmp(action, "double_fault")) {
        // Double fault: cause an exception on a corrupted stack pointer.
        // The kernel's exception entry code saves registers to SP at entry.
        // If SP is unmapped, the save itself triggers a second Data Abort
        // during exception processing → double fault (nested abort).
        //
        // On ARM64 Linux with VMAP_STACK, the guard page mechanism normally
        // handles this gracefully.  By corrupting SP to an arbitrary unmapped
        // address, we bypass the guard-page logic and force a true double fault.
        pr_info("crash_test: triggering double fault (bad SP + abort)\n");
        register unsigned long bad_sp asm("x20") = 0xdead0000;
        asm volatile(
            "mov x21, sp\n\t"       // save original SP in x21
            "mov sp, %[bad]\n\t"    // SP ← unmapped address
            "mov x0, #0\n\t"        // x0 = 0
            "str x0, [x0]\n\t"      // Data Abort (write to address 0x0)
            "mov sp, x21"           // (unreachable) restore SP
            : : [bad] "r"(bad_sp) : "x0", "x21", "memory"
        );

    } else {
        pr_err("crash_test: unknown action '%s'\n", action);
        return -EINVAL;
    }

    return 0; // unreachable after crash
}

static void __exit crash_exit(void) {}
module_init(crash_init);
module_exit(crash_exit);
MODULE_LICENSE("GPL");
