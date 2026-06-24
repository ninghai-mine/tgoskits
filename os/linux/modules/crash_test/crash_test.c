// crash_test.c — 内核崩溃测试模块
// 加载时根据 action 参数触发不同类型的崩溃
#include <linux/module.h>
#include <linux/kernel.h>

static char *action = "null";
module_param(action, charp, 0);
MODULE_PARM_DESC(action, "null | bug | undefined");

static int __init crash_init(void)
{
    pr_info("crash_test: triggering action=%s\n", action);

    if (!strcmp(action, "null")) {
        pr_info("crash_test: NULL pointer dereference\n");
        *(volatile int *)0 = 0;           // Data Abort → OOPS → Panic

    } else if (!strcmp(action, "bug")) {
        pr_info("crash_test: calling BUG()\n");
        BUG();                             // Undefined instr → OOPS → Panic

    } else if (!strcmp(action, "undefined")) {
        pr_info("crash_test: undefined instruction\n");
        asm volatile(".inst 0x00000000");  // Undefined Instruction

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
