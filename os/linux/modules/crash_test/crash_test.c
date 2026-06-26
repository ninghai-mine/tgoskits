// crash_test.c — 内核崩溃测试模块
// 加载时根据 action 参数触发不同类型的崩溃
#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/slab.h>
#include <linux/compiler.h>

static char *action = "null";
module_param(action, charp, 0);
MODULE_PARM_DESC(action, "null | write_null | bug | undefined | double_free | corrupt_mem | stack_overflow | exec_null");

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
