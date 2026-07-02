# memory.rs 模块笔记

这个模块是 `chaos-tests` 里的内存子系统测试模型。它不是完整 OS 内存管理实现，而是把虚拟内存区域、物理页框分配、COW、堆/slab、buddy allocator 等概念用轻量结构拼出来，方便测试其它内核路径。

## 地址和布局

- `phys_to_virt` / `virt_to_phys`：模拟内核 direct-map 区的物理地址和虚拟地址转换，本质是加减 `PHYSICAL_MEMORY_OFFSET`。
- `KERNEL_OFFSET`：用户/内核虚拟地址空间分界，用户地址检查必须保证范围在它下面。
- `MEMORY_OFFSET`：模拟物理内存起点。frame index 转物理地址时使用 `MEMORY_OFFSET + index * PAGE_SIZE`。

## VMA 层

- `VmRegion`：一个半开虚拟地址区间 `[start_addr, start_addr + byte_len)`，带权限、backing offset、merge tag 和轻量引用计数。
- `VmMap`：一组有序且不重叠的 `VmRegion`，类似 VMA table。它只描述哪些虚拟区间存在，不保存页表项，也不保存虚拟页到物理 frame 的完整映射。
- `find` / `find_index` 依赖 `regions` 按 `start_addr` 排序；插入和 split 都要维护这个 invariant。
- `remove_range` 目前是粗粒度删除：只要 region 与目标范围相交，就整段删除，不做 trim。
- `find_free` 是 mmap-style 自动选址，从 `mmap_base` 开始找不冲突且不越过 `KERNEL_OFFSET` 的虚拟空洞。

## 物理页框分配

- `FramePool`：bitmap 风格的物理 frame 池，`frame_is_free[index] == true` 表示对应 frame 空闲。
- `FramePool` 内部 API 返回 frame index；外层 `frame_alloc` / `frame_alloc_contig` 返回模拟物理地址。
- `ZoneInfo`：描述物理 frame 的一个 zone，只约束 PFN 范围并维护 zone-local free count / watermark。
- `defragment_frame_pool` 名字保留了历史意图，但现在只扫描 free bitmap 并返回 free frame 数，不移动 frame，也不会改变碎片状态。

## COW 和地址空间

- `PgFrame`：物理页框元数据，目前只维护引用计数 `rc`，不保存页内容。
- `CowPageMapping`：单个 COW 虚拟页的 side-table entry，包含当前 frame index、共享的 `PgFrame` 元数据、是否 writable、是否仍 pending COW。
- `AddrSpace`：简化地址空间，组合 `VmMap` 和 `cow_pages`。它不是完整页表实现，大多数页映射只有在 COW fault 时才被建模。
- `fork_from`：复制 VMA 层，并通过 `CowPageMapping::clone_for_fork` 让父子共享 frame metadata。
- `handle_cow_fault`：如果页面已被 COW side table 追踪，就让 mapping 自己 resolve；如果没追踪，就懒分配一个 private mapping。
- `tracked_cow_page_count` 不是完整 RSS，只统计 COW side table 里的页。
- `shared_cow_page_count` 统计仍共享的 COW 页数量，不是 sharer 数量。

## Kernel stack 和用户地址检查

- `KernelStack`：用一块 heap allocation 模拟 kernel stack backing storage，只保存 base address，`top()` 返回向低地址增长栈的初始 SP。
- `check_access` / `check_access_rw`：`access_ok` 风格的用户地址范围检查，只判断范围是否留在用户空间，不走页表，也不检查真实 PTE 权限。
- `copy_from_user` / `copy_to_user`：测试桩，只做范围检查，不真实读写用户内存。
- `read_user_fixup`：模拟读用户内存失败时的 fixup 返回点。

## Heap、slab 和 buddy

- `heap_init`：对 heap 起点/大小做页对齐，返回逻辑结束地址；它不真正安装 allocator metadata。
- `heap_grow`：尽量从 `FramePool` 分配 `n` 个 frame，并返回 direct-map 虚拟地址范围。它只和最后一个返回段合并，不保证全局最少区间数。
- `SlabEntry`：简化 slab entry，把一段 `Vec<u8>` 切成固定大小对象槽。分配返回的是 `data` 内的 byte offset，不是 raw pointer。
- `BuddyAllocator`：按 `2^order` 页块管理连续物理页框；`alloc_order` 找目标 order 或更大块并拆分，`free_order` 尝试和 buddy 合并。
- `fragmentation_score`：估算 buddy allocator 的外部碎片，含义是空闲页中不在最大连续空闲块里的比例。

## 主要简化点

- 没有完整 page table；VMA 和 per-page COW side table 是分离的模型。
- COW 不复制真实页内容，只更新 frame index 和引用计数。
- `VmMap::remove_range` / `AddrSpace::protect` 对部分覆盖 region 的处理仍是粗粒度的。
- 部分统计函数是测试启发式，不应理解成真实内核指标。
