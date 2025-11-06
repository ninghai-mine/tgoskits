//! Mock implementations for testing
//!
//! This module provides mock implementations used in tests for the page-table-generic crate.
#![cfg(not(target_os = "none"))]

use page_table_generic::*;
use std::vec::Vec;

mod mocks;

use mocks::*;

#[test]
fn test_pte() {
    let mut want = PteImpl(0);
    want.set_valid(true);
    assert!(want.valid());

    let addr = PhysAddr::from(0xff123456000usize);
    want.set_paddr(addr);
    assert_eq!(want.paddr(), addr);
}

fn test_high<T: TableGeneric, A: FramAllocator>(
    pte: T::P,
    alloc: A,
    test_vaddr: VirtAddr,
    expected_leaf_level: usize,
    test_name: &str,
) where
    T::P: std::fmt::Debug,
{
    let mut pg = PageTable::<T, A>::new(alloc).unwrap();
    println!("table page size: {:#x}", T::PAGE_SIZE);
    println!("valid bits: {}", PageTable::<T, A>::valid_bits());
    println!("\n=== {test_name} 映前状态 - walk_all (遍历所有项) ===");
    for p in pg.walk(WalkConfig {
        start_vaddr: VirtAddr::new(0),
        end_vaddr: VirtAddr::new(usize::MAX),
    }) {
        println!(
            "l: {}, va: {:?}, pte: {:?}, final: {}",
            p.level, p.vaddr, p.pte, p.is_final_mapping
        );
    }

    pg.map(&MapConfig {
        vaddr: test_vaddr,
        paddr: 0x0000usize.into(),
        size: 0x2000,
        pte,
        allow_huge: false,
        flush: false,
    })
    .unwrap();

    println!("\n=== {} 映后状态 - walk_valid结果 ===", test_name);
    let mut count = 0;
    let mut valid_entries = Vec::new();
    for p in pg.walk_valid() {
        println!("l: {}, va: {:?}, pte: {:?}", p.level, p.vaddr, p.pte);
        valid_entries.push((p.vaddr, p.pte, p.level));
        count += 1;
    }

    // 注意：walk_valid()只返回叶子级别的有效条目，所以是2个
    // 我们期望的5个条目来自自定义walker，包括中间级别
    println!("walk_valid() 返回 {count} 个叶子级别条目");

    println!(
        "\n=== {} 映后状态 - 显示完整层次（所有有效项） ===",
        test_name
    );
    for p in pg.walk(WalkConfig {
        start_vaddr: VirtAddr::new(0),
        end_vaddr: VirtAddr::new(usize::MAX),
    }) {
        println!(
            "l: {}, va: {:?}, c: PTE PA: {:?} Block: {}, Final: {}",
            p.level,
            p.vaddr,
            p.pte.paddr(),
            p.pte.is_huge(),
            p.is_final_mapping
        );
    }

    assert_eq!(count, 2); // walk_valid() 应该返回2个叶子级别条目

    // === 严格的地址和属性验证 ===

    // 验证虚拟地址：映射从指定地址开始的0x2000字节（2个4KB页面）
    let expected_vaddrs = [test_vaddr, VirtAddr::new(test_vaddr.raw() + 0x1000)];

    // 验证虚拟地址映射正确
    for (i, (vaddr, pte, level)) in valid_entries.iter().enumerate() {
        assert_eq!(
            *vaddr, expected_vaddrs[i],
            "{} 第{}个条目的虚拟地址不匹配，期望 {:?}，实际 {:?}",
            test_name, i, expected_vaddrs[i], vaddr
        );

        // 验证这是叶子级别（使用参数化的期望级别）
        assert_eq!(
            *level, expected_leaf_level,
            "{} 叶子级别页表项应该在level {}，实际在level {level}",
            test_name, expected_leaf_level
        );

        // 验证页表项是有效的
        assert!(pte.valid(), "{} 页表项应该是有效的", test_name);

        // 验证不是大页（因为allow_huge=false且页面大小为4KB）
        assert!(!pte.is_huge(), "{} 页表项不应该是大页", test_name);

        // 物理地址偏移验证：由于内存分配的随机性，我们只验证相对关系

        // 注意：由于内存分配的随机性，我们只验证物理地址的偏移部分
        // 实际的物理基地址可能不同，但偏移应该是固定的
        let actual_paddr = pte.paddr();
        let actual_offset = actual_paddr.raw() % 0x1000; // 页内偏移
        assert_eq!(
            actual_offset, 0,
            "{} 页内偏移应该是0，实际是 {actual_offset:?}",
            test_name
        );

        // 验证两个页表项的物理地址相差0x1000（4KB）
        if i > 0 {
            let prev_pte = &valid_entries[i - 1].1;
            let prev_paddr = prev_pte.paddr();
            let addr_diff = actual_paddr.raw().saturating_sub(prev_paddr.raw());
            assert_eq!(
                addr_diff, 0x1000,
                "{} 相邻页面物理地址应该相差0x1000，实际相差 {addr_diff:?}",
                test_name
            );
        }

        println!(
            "✓ {} 页面{}验证通过: VA={:?}, PA={:?}, Level={}, Valid={}, Huge={}",
            test_name,
            i,
            vaddr,
            actual_paddr,
            level,
            pte.valid(),
            pte.is_huge()
        );
    }

    println!("🎉 {} 所有地址和属性验证通过！", test_name);
}

#[test]
fn test_new() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Trace)
        .try_init();

    test_high::<T4kL4, Fram4k>(
        PteImpl(0),
        Fram4k,
        0x0000f00000000000usize.into(), // 高虚拟地址
        1,                              // 叶子级别
        "T4kL4",
    );
}

#[test]
fn test_new_l3() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Trace)
        .try_init();

    test_high::<T4kL3, Fram4k>(
        PteImpl(0),
        Fram4k,
        0x0000000000000000usize.into(), // 低虚拟地址
        1,                              // 叶子级别
        "T4kL3",
    );
}

#[test]
fn test_new_l5() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Trace)
        .try_init();

    test_high::<T4kL5, Fram4k>(
        PteImpl(0),
        Fram4k,
        0x0000f00000000000usize.into(), // 高虚拟地址
        1,                              // 叶子级别
        "T4kL5",
    );
}
