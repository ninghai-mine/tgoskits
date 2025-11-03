/// 龙架构DMWIN（Direct Mapping Window）设置宏
/// 设置直接映射窗口寄存器，用于内存地址映射
macro_rules! setup_dmwins {
    ($temp: expr) => {
        concat!(
            // 设置DMWIN0: WUC, PLV0, 0x8000 xxxx xxxx xxxx
            "li.d  ",
            stringify!($temp),
            ", 0x8000000000000000\n",
            "csrwr   ",
            stringify!($temp),
            ", 0x180\n", // LOONGARCH_CSR_DMWIN0
            // 设置DMWIN1: CAC, PLV0, 0x9000 xxxx xxxx xxxx
            "li.d    ",
            stringify!($temp),
            ", 0x9000000000000000\n",
            "csrwr   ",
            stringify!($temp),
            ", 0x181\n", // LOONGARCH_CSR_DMWIN1
            // 设置DMWIN2: WUC, PLV0, 0xa000 xxxx xxxx xxxx
            "li.d    ",
            stringify!($temp),
            ", 0xa000000000000000\n",
            "csrwr   ",
            stringify!($temp),
            ", 0x182\n", // LOONGARCH_CSR_DMWIN2
            // 设置DMWIN3: 0x0, unused
            "li.d    ",
            stringify!($temp),
            ", 0x0\n",
            "csrwr   ",
            stringify!($temp),
            ", 0x183\n", // LOONGARCH_CSR_DMWIN3
        )
    };
}

macro_rules! jump_virt_addr {
    ($temp1: expr, $temp2: expr) => {
        concat!( "li.d ", stringify!($temp1), ", 0x9000000000000000\n",
        "pcaddi ", stringify!($temp2), ", 0",
        "bstrins.d ", stringify!($temp1),", ", stringify!($temp2), ", (48 - 1), 0\n",
        "jirl zero, ", stringify!($temp1), ", 0xc")
    };
}
