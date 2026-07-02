use crate::*;

/// Convert a physical address into the kernel's direct-map virtual address.
///
/// The test kernel models physical memory as a linear mapping that starts at
/// `PHYSICAL_MEMORY_OFFSET`, so this is an offset calculation rather than a page-table walk.
pub fn phys_to_virt(phys_addr: usize) -> usize {
    PHYSICAL_MEMORY_OFFSET.wrapping_add(phys_addr)
}

/// Convert a kernel direct-map virtual address back into a physical address.
///
/// This is only meaningful for virtual addresses produced by `phys_to_virt`.
pub fn virt_to_phys(virt_addr: usize) -> usize {
    virt_addr.wrapping_sub(PHYSICAL_MEMORY_OFFSET)
}

/// Return the offset of a kernel virtual address from the kernel image base.
pub fn kernel_offset(virt_addr: usize) -> usize {
    virt_addr.wrapping_sub(KERNEL_OFFSET)
}

pub struct VmRegion {
    /// Start virtual address of this half-open region.
    pub start_addr: usize,
    /// Region length in bytes; the covered address range is [start_addr, start_addr + byte_len).
    pub byte_len: usize,
    /// Access permissions and mapping attributes such as VM_READ or VM_GROWSDOWN.
    pub vm_flags: u32,
    /// Offset in the backing object that corresponds to `start_addr`.
    pub backing_offset: usize,
    /// Extra classification tag; regions with different tags must not be merged.
    pub merge_tag: u16,
    /// Lightweight reference count used by fork/COW-style region sharing.
    pub ref_count: AtomicUsize,
}

impl VmRegion {
    pub fn new(start_addr: usize, byte_len: usize, vm_flags: u32) -> Self {
        Self {
            start_addr,
            byte_len,
            vm_flags,
            backing_offset: 0,
            merge_tag: 0,
            ref_count: AtomicUsize::new(1),
        }
    }

    pub fn with_backing_offset(
        start_addr: usize,
        byte_len: usize,
        vm_flags: u32,
        backing_offset: usize,
    ) -> Self {
        Self {
            start_addr,
            byte_len,
            vm_flags,
            backing_offset,
            merge_tag: 0,
            ref_count: AtomicUsize::new(1),
        }
    }

    pub fn end(&self) -> usize {
        self.start_addr + self.byte_len
    }

    pub fn contains(&self, virt_addr: usize) -> bool {
        virt_addr >= self.start_addr && virt_addr < self.start_addr + self.byte_len
    }

    pub fn overlaps(&self, other: &VmRegion) -> bool {
        let self_end = self.start_addr.wrapping_add(self.byte_len);
        let other_end = other.start_addr.wrapping_add(other.byte_len);
        // Regions are half-open ranges: [start_addr, end). Adjacent regions do not overlap.
        let no_overlap = self_end <= other.start_addr || other_end <= self.start_addr;
        !no_overlap
    }

    pub fn split_at(&self, split_addr: usize) -> Option<(VmRegion, VmRegion)> {
        let region_end = self.start_addr + self.byte_len;
        if split_addr <= self.start_addr || split_addr >= region_end {
            return None;
        }
        let left_byte_len = split_addr - self.start_addr;
        let right_byte_len = self.byte_len - left_byte_len;
        let left_backing_offset = self.backing_offset;
        let right_backing_offset = self.backing_offset.wrapping_add(left_byte_len);
        let mut left_vm_flags = self.vm_flags;
        let right_vm_flags = self.vm_flags;
        if self.vm_flags & VM_GROWSDOWN != 0 {
            // Only the higher-address side keeps grow-down behavior after a stack split.
            left_vm_flags &= !VM_GROWSDOWN;
        }
        let left_region = VmRegion {
            start_addr: self.start_addr,
            byte_len: left_byte_len,
            vm_flags: left_vm_flags,
            backing_offset: left_backing_offset,
            merge_tag: self.merge_tag,
            ref_count: AtomicUsize::new(self.ref_count.load(Ordering::Relaxed)),
        };
        let right_region = VmRegion {
            start_addr: split_addr,
            byte_len: right_byte_len,
            vm_flags: right_vm_flags,
            backing_offset: right_backing_offset,
            merge_tag: self.merge_tag,
            ref_count: AtomicUsize::new(self.ref_count.load(Ordering::Relaxed)),
        };
        Some((left_region, right_region))
    }

    pub fn merge_with(&self, other: &VmRegion) -> Option<VmRegion> {
        let self_end = self.start_addr + self.byte_len;
        if self_end != other.start_addr {
            return None;
        }
        if self.vm_flags != other.vm_flags {
            return None;
        }
        if self.merge_tag != other.merge_tag {
            return None;
        }
        if self.backing_offset.wrapping_add(self.byte_len) != other.backing_offset {
            return None;
        }
        let merged_region = VmRegion {
            start_addr: self.start_addr,
            byte_len: self.byte_len + other.byte_len,
            vm_flags: self.vm_flags,
            backing_offset: self.backing_offset,
            merge_tag: self.merge_tag,
            ref_count: AtomicUsize::new(
                self.ref_count
                    .load(Ordering::Relaxed)
                    .max(other.ref_count.load(Ordering::Relaxed)),
            ),
        };
        Some(merged_region)
    }

    pub fn increment_ref_count(&self) -> usize {
        self.ref_count.fetch_add(1, Ordering::Relaxed)
    }
    pub fn decrement_ref_count(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::Relaxed)
    }
    pub fn current_ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Relaxed)
    }
}

pub struct VmMap {
    /// Sorted, non-overlapping virtual memory regions in this address space.
    ///
    /// This is a VMA-style layout table: it records which virtual address
    /// ranges exist and what attributes they have. It does not store page-table
    /// entries or virtual-page to physical-frame mappings.
    pub regions: Vec<VmRegion>,
    /// Current program break; the simulated heap grows up to this address.
    pub brk: usize,
    /// Preferred starting point when searching for automatically placed mmap regions.
    pub mmap_base: usize,
}

impl VmMap {
    /// Create an empty virtual layout with fixed test-model heap and mmap bases.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            brk: 0x0040_0000,
            mmap_base: 0x7000_0000,
        }
    }

    /// Insert a region while preserving the sorted, non-overlapping invariant.
    ///
    /// Adjacent regions are deliberately left separate here; `merge_with` models
    /// the checks a separate coalescing pass would need.
    pub fn insert(&mut self, region: VmRegion) -> Result<(), &'static str> {
        let new_start = region.start_addr;
        let new_end = new_start.wrapping_add(region.byte_len);
        let mut insert_index = 0;
        while insert_index < self.regions.len() {
            let existing_start = self.regions[insert_index].start_addr;
            let existing_end = existing_start + self.regions[insert_index].byte_len;
            if new_start < existing_end && existing_start < new_end {
                return Err("overlap");
            }
            if existing_start > new_start {
                break;
            }
            insert_index += 1;
        }
        // Insert keeps adjacent regions separate; this records whether an
        // explicit coalescing pass could merge the new region with a neighbor.
        let _can_coalesce_with_neighbor = (insert_index > 0
            && self.regions[insert_index - 1].merge_with(&region).is_some())
            || (insert_index < self.regions.len()
                && region.merge_with(&self.regions[insert_index]).is_some());

        self.regions.insert(insert_index, region);
        Ok(())
    }

    /// Find the region containing a virtual address.
    ///
    /// This uses binary search because `insert` keeps `regions` sorted by
    /// `start_addr`.
    pub fn find(&self, virt_addr: usize) -> Option<&VmRegion> {
        self.find_index(virt_addr)
            .map(|region_index| &self.regions[region_index])
    }

    /// Find the index of the region containing a virtual address.
    ///
    /// Returning the index lets callers update or split the region while still
    /// reusing the same binary-search logic as `find`.
    pub fn find_index(&self, virt_addr: usize) -> Option<usize> {
        let region_count = self.regions.len();
        if region_count == 0 {
            return None;
        }
        let mut low = 0;
        let mut high = region_count;
        while low < high {
            let mid = low + (high - low) / 2;
            let region = &self.regions[mid];
            if virt_addr < region.start_addr {
                high = mid;
            } else if virt_addr >= region.start_addr + region.byte_len {
                low = mid + 1;
            } else {
                return Some(mid);
            }
        }
        None
    }

    /// Remove every region that intersects the target range.
    ///
    /// This is coarse-grained: partially overlapping regions are removed
    /// entirely instead of being trimmed or split.
    pub fn remove_range(&mut self, start_addr: usize, byte_len: usize) -> usize {
        let end_addr = start_addr.wrapping_add(byte_len);
        let before = self.regions.len();
        let mut index = 0;
        while index < self.regions.len() {
            let region_start = self.regions[index].start_addr;
            let region_end = region_start + self.regions[index].byte_len;
            if region_start >= start_addr && region_end <= end_addr {
                self.regions.remove(index);
            } else if region_start < end_addr && region_end > start_addr {
                self.regions.remove(index);
            } else {
                index += 1;
            }
        }
        before - self.regions.len()
    }

    /// Find an unmapped virtual range for an automatically placed mmap-style mapping.
    ///
    /// The search starts at `mmap_base`, respects the requested alignment, and
    /// rejects candidates that would cross into the kernel virtual range.
    pub fn find_free(&self, byte_len: usize, align: usize) -> Option<usize> {
        if byte_len == 0 {
            return Some(self.mmap_base);
        }
        let alignment = if align > 1 { align } else { PAGE_SIZE };
        let align_mask = alignment - 1;
        let mut candidate_start = (self.mmap_base + align_mask) & !align_mask;
        let mut iterations = 0;
        let max_iters = self.regions.len() + 2;
        while iterations < max_iters {
            if candidate_start.wrapping_add(byte_len) > KERNEL_OFFSET
                || candidate_start.wrapping_add(byte_len) < candidate_start
            {
                return None;
            }
            let candidate_end = candidate_start + byte_len;
            let mut conflict_end = 0usize;
            let mut has_conflict = false;
            for region in self.regions.iter() {
                let region_start = region.start_addr;
                let region_end = region_start + region.byte_len;
                if region_start < candidate_end && candidate_start < region_end {
                    conflict_end = region_end;
                    has_conflict = true;
                    break;
                }
            }
            if !has_conflict {
                return Some(candidate_start);
            }
            candidate_start = (conflict_end + align_mask) & !align_mask;
            iterations += 1;
        }
        None
    }

    /// Total bytes covered by all virtual regions.
    pub fn total_mapped(&self) -> usize {
        let mut total_bytes = 0usize;
        for region in self.regions.iter() {
            total_bytes = total_bytes.wrapping_add(region.byte_len);
        }
        total_bytes
    }

    /// Clone the region metadata, including each region's current ref count value.
    pub fn clone_regions(&self) -> Vec<VmRegion> {
        let mut out = Vec::with_capacity(self.regions.len());
        for region in self.regions.iter() {
            let cloned_region = VmRegion {
                start_addr: region.start_addr,
                byte_len: region.byte_len,
                vm_flags: region.vm_flags,
                backing_offset: region.backing_offset,
                merge_tag: region.merge_tag,
                ref_count: AtomicUsize::new(region.ref_count.load(Ordering::Relaxed)),
            };
            out.push(cloned_region);
        }
        out
    }

    /// Return the unmapped gap after a region, bounded by `KERNEL_OFFSET`.
    pub fn gap_after(&self, region_index: usize) -> usize {
        if region_index >= self.regions.len() {
            return 0;
        }
        let region_end =
            self.regions[region_index].start_addr + self.regions[region_index].byte_len;
        if region_index + 1 < self.regions.len() {
            self.regions[region_index + 1]
                .start_addr
                .saturating_sub(region_end)
        } else {
            KERNEL_OFFSET.saturating_sub(region_end)
        }
    }
}

pub struct ZoneInfo {
    /// Logical zone identifier, such as ZONE_DMA or ZONE_NORMAL.
    pub zone_id: usize,
    /// First physical frame number covered by this zone.
    pub base_pfn: usize,
    /// Number of physical frames in this zone.
    pub page_count: usize,
    /// Current number of free frames in this zone.
    pub free_count: AtomicUsize,
    /// Below this free-page watermark, normal allocations should stop.
    pub low_watermark: usize,
    /// Target free-page watermark used to measure pressure and reclaim work.
    pub high_watermark: usize,
    /// Whether this zone is managed by the simulated allocator.
    pub managed: AtomicBool,
}

impl ZoneInfo {
    /// Create a physical-memory zone over the PFN range [base, base + count).
    pub fn new(id: usize, base: usize, count: usize, low: usize, high: usize) -> Self {
        Self {
            zone_id: id,
            base_pfn: base,
            page_count: count,
            free_count: AtomicUsize::new(count),
            low_watermark: low,
            high_watermark: high,
            managed: AtomicBool::new(true),
        }
    }

    /// Return whether the zone has enough free pages for ordinary allocation.
    pub fn zone_can_alloc(&self) -> bool {
        self.free_count.load(Ordering::Relaxed) > self.low_watermark
    }

    /// Estimate memory pressure as a percentage between the high and low watermarks.
    pub fn zone_pressure(&self) -> usize {
        let free = self.free_count.load(Ordering::Relaxed);
        if free >= self.high_watermark {
            return 0;
        }
        if free <= self.low_watermark {
            return 100;
        }
        let range = self.high_watermark - self.low_watermark;
        let deficit = self.high_watermark - free;
        (deficit * 100) / range
    }

    /// Number of pages that should be reclaimed to return to the high watermark.
    pub fn reclaim_target(&self) -> usize {
        let free = self.free_count.load(Ordering::Relaxed);
        if free >= self.high_watermark {
            return 0;
        }
        self.high_watermark - free
    }

    /// Return whether a physical frame number belongs to this zone.
    pub fn contains_pfn(&self, pfn: usize) -> bool {
        pfn >= self.base_pfn && pfn < self.base_pfn + self.page_count
    }
}

pub struct FramePool {
    /// Per-frame free bitmap for the simulated physical frame pool.
    ///
    /// `true` means free, `false` means allocated. The index is the PFN/frame id.
    pub(crate) frame_is_free: Mutex<Vec<bool>>,
    /// Total number of frames represented by `frame_is_free`.
    pub(crate) frame_count: usize,
}

impl FramePool {
    /// Create a frame pool with all frames initially free.
    pub fn new(frame_count: usize) -> Self {
        Self {
            frame_is_free: Mutex::new(vec![true; frame_count]),
            frame_count,
        }
    }

    /// Allocate one frame while holding the simulated global kernel lock.
    ///
    /// Returns a frame index, not a physical address.
    pub fn alloc_frame_index_with_kernel_lock(&self, lock_owner_id: usize) -> Option<usize> {
        GLOBAL_KERNEL_LOCK.enter(lock_owner_id);
        let frame_index = self.alloc_frame_index();
        GLOBAL_KERNEL_LOCK.leave();
        frame_index
    }

    /// Allocate the first available frame and return its frame index.
    pub fn alloc_frame_index(&self) -> Option<usize> {
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        for (frame_index, is_free) in frame_is_free.iter_mut().enumerate() {
            if *is_free {
                *is_free = false;
                return Some(frame_index);
            }
        }
        None
    }

    /// Allocate `frame_count` contiguous frames whose start index is aligned to `2^align_log2`.
    ///
    /// Returns the starting frame index.
    pub fn alloc_contiguous_frames(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        if frame_count == 0 || align_log2 >= usize::BITS as usize {
            return None;
        }
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        let frame_alignment = 1usize << align_log2;
        for start_index in (0..frame_is_free.len()).step_by(frame_alignment) {
            let end_index = match start_index.checked_add(frame_count) {
                Some(end_index) if end_index <= frame_is_free.len() => end_index,
                Some(_) => break,
                None => return None,
            };
            if (start_index..end_index).all(|frame_index| frame_is_free[frame_index]) {
                for frame_index in start_index..end_index {
                    frame_is_free[frame_index] = false;
                }
                return Some(start_index);
            }
        }
        None
    }

    /// Mark a frame index as free.
    pub fn free_frame_index(&self, frame_index: usize) {
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        if frame_index < frame_is_free.len() {
            frame_is_free[frame_index] = true;
        }
    }

    /// Return whether a frame index is currently free.
    pub fn is_frame_free(&self, frame_index: usize) -> bool {
        let frame_is_free = self.frame_is_free.lock().unwrap();
        frame_index < frame_is_free.len() && frame_is_free[frame_index]
    }

    /// Count all currently free frames in the whole pool.
    pub fn free_frame_count(&self) -> usize {
        self.frame_is_free
            .lock()
            .unwrap()
            .iter()
            .filter(|&&is_free| is_free)
            .count()
    }

    /// Allocate one free frame from the PFN range described by `zone`.
    ///
    /// The pool owns the actual free bitmap; `zone` only constrains the PFN
    /// range and tracks zone-local watermarks/counts.
    pub fn alloc_frame_index_from_zone(&self, zone: &ZoneInfo) -> Option<usize> {
        if !zone.zone_can_alloc() {
            return None;
        }
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        let zone_start = zone.base_pfn;
        let zone_end = zone_start + zone.page_count;
        for frame_index in zone_start..min(zone_end, frame_is_free.len()) {
            if frame_is_free[frame_index] {
                frame_is_free[frame_index] = false;
                zone.free_count.fetch_sub(1, Ordering::Relaxed);
                return Some(frame_index);
            }
        }
        None
    }

    /// Return a frame to both the global pool and the zone-local free count.
    pub fn free_frame_index_to_zone(&self, frame_index: usize, zone: &ZoneInfo) {
        if !zone.contains_pfn(frame_index) {
            return;
        }
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        // Only return frames that are inside this zone and currently allocated;
        // otherwise zone.free_count can drift on cross-zone or duplicate frees.
        if frame_index < frame_is_free.len() && !frame_is_free[frame_index] {
            frame_is_free[frame_index] = true;
            zone.free_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Allocate up to `requested_count` frames and return their frame indices.
    ///
    /// This is a partial allocation API: if fewer than `count` frames are free,
    /// it returns the frames it did acquire and does not roll back.
    pub fn alloc_frame_indices_at_most(&self, requested_count: usize) -> Vec<usize> {
        let mut frame_is_free = self.frame_is_free.lock().unwrap();
        let mut allocated_indices = Vec::with_capacity(requested_count);
        for (frame_index, is_free) in frame_is_free.iter_mut().enumerate() {
            if allocated_indices.len() >= requested_count {
                break;
            }
            if *is_free {
                *is_free = false;
                allocated_indices.push(frame_index);
            }
        }
        allocated_indices
    }
}

pub fn frame_alloc(pool: &FramePool) -> Option<usize> {
    // FramePool is the low-level bitmap allocator, so it returns a frame index.
    // The rCore-style public frame allocator returns the simulated physical
    // address covered by that frame.
    pool.alloc_frame_index().and_then(|frame_index| {
        frame_index
            .checked_mul(PAGE_SIZE)
            .and_then(|frame_offset| MEMORY_OFFSET.checked_add(frame_offset))
    })
}

pub fn frame_dealloc(pool: &FramePool, phys_addr: usize) {
    // Public deallocation receives the simulated physical address returned by
    // frame_alloc/frame_alloc_contig. Convert it back to the underlying frame
    // index before touching the FramePool bitmap.
    let frame_offset = match phys_addr.checked_sub(MEMORY_OFFSET) {
        Some(frame_offset) => frame_offset,
        None => return,
    };
    if frame_offset % PAGE_SIZE != 0 {
        return;
    }
    pool.free_frame_index(frame_offset / PAGE_SIZE);
}

pub fn frame_alloc_contig(
    pool: &FramePool,
    frame_count: usize,
    align_log2: usize,
) -> Option<usize> {
    // The FramePool API returns the starting frame index for the contiguous
    // run; this wrapper exposes the physical address of that first frame.
    pool.alloc_contiguous_frames(frame_count, align_log2)
        .and_then(|start_index| {
            start_index
                .checked_mul(PAGE_SIZE)
                .and_then(|frame_offset| MEMORY_OFFSET.checked_add(frame_offset))
        })
}

/// Metadata for one simulated physical page frame.
///
/// This does not store page contents. It only tracks how many mappings or
/// owners currently refer to the frame, which is enough for the COW tests in
/// this crate.
pub struct PgFrame {
    /// Reference count for this frame.
    ///
    /// A count above 1 means the frame is shared and a write fault should
    /// allocate a private copy. A count of 0 means the frame should not be
    /// resurrected by `inc_if_nonzero`.
    pub rc: AtomicUsize,
}

impl PgFrame {
    /// Create frame metadata with no active references.
    pub fn new() -> Self {
        Self {
            rc: AtomicUsize::new(0),
        }
    }

    /// Create frame metadata with an explicit initial reference count.
    pub fn with_rc(n: usize) -> Self {
        Self {
            rc: AtomicUsize::new(n),
        }
    }

    /// Increment the reference count and return the previous value.
    pub fn up(&self) -> usize {
        let prev = self.rc.fetch_add(1, Ordering::Relaxed);
        prev
    }

    /// Decrement the reference count and return the previous value.
    pub fn down(&self) -> usize {
        let prev = self.rc.fetch_sub(1, Ordering::Relaxed);
        prev
    }

    /// Return the current reference count.
    pub fn count(&self) -> usize {
        // This is only an instantaneous atomic read of the reference count.
        // It is not a stable snapshot if other threads are updating the count.
        self.rc.load(Ordering::Relaxed)
    }

    /// Replace the reference count unconditionally.
    pub fn set(&self, n: usize) {
        let _old = self.rc.swap(n, Ordering::Relaxed);
    }

    /// Change the count only if it still matches `expected`.
    pub fn cas(&self, expected: usize, desired: usize) -> bool {
        self.rc
            .compare_exchange(expected, desired, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// Try to take one more reference without reviving a zero-count frame.
    ///
    /// This is useful when zero means the frame is already being released. The
    /// CAS loop retries if another thread changes the count first.
    pub fn inc_if_nonzero(&self) -> bool {
        let mut current = self.rc.load(Ordering::Relaxed);
        while current != 0 {
            let next = match current.checked_add(1) {
                Some(next) => next,
                None => return false,
            };
            match self
                .rc
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return true,
                // Another thread changed the count first. Retry from the
                // observed value instead of doing a separate load.
                Err(observed) => current = observed,
            }
        }
        false
    }
}

/// COW state for one shared virtual page.
///
/// This is a small test model of a page-table entry: it records which frame a
/// virtual page currently maps to and whether a pending COW write fault still
/// needs to be resolved. It does not copy page contents or install real PTEs.
pub struct CowPageMapping {
    /// Current physical frame index for this virtual page.
    pub frame_index: AtomicUsize,
    /// Reference-count metadata for the frame currently mapped here.
    ///
    /// This is shared across forked address spaces until a COW fault installs a
    /// private frame.
    pub frame_meta: Mutex<Arc<PgFrame>>,
    /// Whether this mapping is writable after COW resolution.
    pub is_writable: AtomicBool,
    /// Whether the next write still needs to allocate a private frame.
    pub cow_pending: AtomicBool,
}

/// Backward-compatible name for older tests and call sites.
pub type SharedPage = CowPageMapping;

impl CowPageMapping {
    /// Create a COW-pending mapping to `initial_frame_index`.
    pub fn new(initial_frame_index: usize) -> Self {
        Self::new_cow(initial_frame_index, Arc::new(PgFrame::with_rc(1)))
    }

    /// Create a COW-pending mapping with shared frame metadata.
    pub fn new_cow(initial_frame_index: usize, frame_meta: Arc<PgFrame>) -> Self {
        Self {
            frame_index: AtomicUsize::new(initial_frame_index),
            frame_meta: Mutex::new(frame_meta),
            is_writable: AtomicBool::new(false),
            cow_pending: AtomicBool::new(true),
        }
    }

    /// Create a private writable mapping with a single frame reference.
    pub fn new_private(initial_frame_index: usize) -> Self {
        Self {
            frame_index: AtomicUsize::new(initial_frame_index),
            frame_meta: Mutex::new(Arc::new(PgFrame::with_rc(1))),
            is_writable: AtomicBool::new(true),
            cow_pending: AtomicBool::new(false),
        }
    }

    /// Create a child mapping for fork and mark the parent mapping COW-pending.
    pub fn clone_for_fork(&self) -> Self {
        self.is_writable.store(false, Ordering::Relaxed);
        self.cow_pending.store(true, Ordering::Relaxed);

        let frame_meta = {
            let frame_meta = self.frame_meta.lock().unwrap();
            frame_meta.up();
            Arc::clone(&*frame_meta)
        };
        Self::new_cow(self.current_frame_index(), frame_meta)
    }

    /// Resolve a COW write fault and return the frame index now used here.
    ///
    /// If the mapping has already been resolved, no new frame is allocated. If
    /// it is still pending, this mapping switches to a newly allocated private
    /// frame and the old frame's reference count is decremented.
    pub fn resolve_cow_fault(&self, pool: &FramePool) -> Result<usize, &'static str> {
        let current_frame_index = self.frame_index.load(Ordering::Relaxed);
        if !self.cow_pending.load(Ordering::Relaxed) {
            return Ok(current_frame_index);
        }

        let mut frame_meta = self.frame_meta.lock().unwrap();
        if frame_meta.count() <= 1 {
            self.is_writable.store(true, Ordering::Relaxed);
            self.cow_pending.store(false, Ordering::Relaxed);
            return Ok(current_frame_index);
        }

        let private_frame_index = pool.alloc_frame_index().ok_or("oom")?;
        self.frame_index
            .store(private_frame_index, Ordering::Relaxed);
        frame_meta.down();
        *frame_meta = Arc::new(PgFrame::with_rc(1));
        self.is_writable.store(true, Ordering::Relaxed);
        self.cow_pending.store(false, Ordering::Relaxed);
        Ok(private_frame_index)
    }

    /// Compatibility wrapper for the older, less explicit method name.
    pub fn fault(&self, pool: &FramePool) -> Result<usize, &'static str> {
        self.resolve_cow_fault(pool)
    }

    /// Return whether the COW fault has already switched this mapping private.
    pub fn is_cow_resolved(&self) -> bool {
        !self.cow_pending.load(Ordering::Relaxed) && self.is_writable.load(Ordering::Relaxed)
    }

    /// Return the current frame index.
    pub fn current_frame_index(&self) -> usize {
        self.frame_index.load(Ordering::Relaxed)
    }

    /// Return the current frame reference count.
    pub fn frame_ref_count(&self) -> usize {
        self.frame_meta.lock().unwrap().count()
    }

    /// Drop this mapping's reference to its current frame metadata.
    pub fn release_frame_reference(&self) {
        self.frame_meta.lock().unwrap().down();
    }

    /// Compatibility wrapper for the older name.
    pub fn frame_id(&self) -> usize {
        self.current_frame_index()
    }
}
/// Simulated kernel stack backing storage.
///
/// A real kernel stack is a mapped virtual address range. In these tests we
/// model it with one heap allocation and keep the allocation's base address so
/// code can reason about stack addresses.
pub struct KernelStack {
    /// Start address of the heap allocation backing this simulated stack.
    base_addr: usize,
}

impl KernelStack {
    /// Allocate zeroed backing memory for one kernel stack.
    pub fn new() -> Self {
        let stack_bytes = vec![0u8; KSTACK_SIZE].into_boxed_slice();
        let base_addr = Box::into_raw(stack_bytes) as *mut u8 as usize;
        Self { base_addr }
    }

    /// Return the initial stack pointer for a downward-growing stack.
    pub fn top(&self) -> usize {
        self.base_addr + KSTACK_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        unsafe {
            // Rebuild the Box so Rust can release the heap allocation that was
            // intentionally leaked by Box::into_raw in `new`.
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(
                self.base_addr as *mut u8,
                KSTACK_SIZE,
            ));
        }
    }
}

/// Return whether `[addr, addr + len)` stays entirely in user space.
///
/// This is a lightweight `access_ok`-style range check for syscall buffers. It
/// only rejects kernel addresses and ranges that cross into kernel space; it
/// does not walk page tables or check per-page permissions.
pub fn check_access(addr: usize, len: usize) -> bool {
    if addr >= KERNEL_OFFSET {
        return false;
    }
    len < KERNEL_OFFSET - addr
}

/// Return whether a user range is valid for a read or write-style access.
///
/// `_writable` documents the caller's intent, but this test model currently
/// performs the same address-range validation for reads and writes.
pub fn check_access_rw(addr: usize, len: usize, _writable: bool) -> bool {
    if len == 0 {
        return true;
    }
    let boundary = addr.wrapping_add(len);
    let crosses_kern = boundary >= KERNEL_OFFSET || boundary < addr;
    if crosses_kern {
        return false;
    }
    boundary < KERNEL_OFFSET
}

/// Simulate copying a value from a user pointer into the kernel.
///
/// `len == 0` means "use the size of `T`". This helper only validates that the
/// source range is in user space; it does not dereference `addr`, so a valid
/// copy returns `T::default()` rather than real user memory contents.
pub fn copy_from_user<T: Copy + Default>(addr: usize, len: usize) -> Option<T> {
    let effective_len = if len == 0 {
        std::mem::size_of::<T>()
    } else {
        len
    };
    if !check_access(addr, effective_len) {
        return None;
    }
    Some(T::default())
}

/// Simulate copying a kernel value into a user pointer.
///
/// `len == 0` means "use the size of `T`". This helper only validates that the
/// destination range is in user space and returns whether the simulated copy
/// would be allowed; it does not write `value` anywhere.
pub fn copy_to_user<T: Copy>(addr: usize, len: usize, _value: &T) -> bool {
    let effective_len = if len == 0 {
        std::mem::size_of::<T>()
    } else {
        len
    };
    check_access_rw(addr, effective_len, true)
}

/// Simulated fixup return for a failed read from user memory.
pub fn read_user_fixup() -> usize {
    1
}

pub fn heap_init(base: usize, sz: usize) -> usize {
    let aligned_base = (base + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let aligned_sz = sz & !(PAGE_SIZE - 1);
    let end = aligned_base + aligned_sz;
    let _metadata_pages = (aligned_sz / PAGE_SIZE + 63) / 64;
    end
}

pub fn heap_grow(pool: &FramePool, n: usize) -> Vec<(usize, usize)> {
    // Return newly acquired direct-map virtual ranges as (start_addr, byte_len).
    let mut addrs: Vec<(usize, usize)> = Vec::new();
    let mut attempts = 0;
    let max_attempts = n * 2;
    let mut acquired = 0;
    while acquired < n && attempts < max_attempts {
        attempts += 1;
        let slot = {
            let mut frame_is_free = pool.frame_is_free.lock().unwrap();
            let mut found_frame_index = None;
            let preferred_start = if addrs.is_empty() {
                0
            } else {
                let (last_va, last_sz) = addrs.last().unwrap();
                let last_pg =
                    (*last_va - PHYSICAL_MEMORY_OFFSET) / PAGE_SIZE + *last_sz / PAGE_SIZE;
                last_pg
            };
            for scan_offset in 0..frame_is_free.len() {
                let frame_index = (preferred_start + scan_offset) % frame_is_free.len();
                if frame_is_free[frame_index] {
                    frame_is_free[frame_index] = false;
                    found_frame_index = Some(frame_index);
                    break;
                }
            }
            found_frame_index
        };
        match slot {
            Some(pg) => {
                let va = PHYSICAL_MEMORY_OFFSET + pg * PAGE_SIZE;
                let mut merged = false;
                // Only merge with the most recently returned range. This keeps
                // common sequential growth compact, but does not guarantee a
                // globally minimal set of ranges.
                if let Some(last) = addrs.last_mut() {
                    if last.0 + last.1 == va {
                        last.1 += PAGE_SIZE;
                        merged = true;
                    } else if va + PAGE_SIZE == last.0 {
                        last.0 = va;
                        last.1 += PAGE_SIZE;
                        merged = true;
                    }
                }
                if !merged {
                    addrs.push((va, PAGE_SIZE));
                }
                acquired += 1;
            }
            None => break,
        }
    }
    let _frag = addrs.len();
    addrs
}

/// A simplified slab allocator entry for fixed-size objects.
///
/// The backing storage is one byte vector split into `capacity` equal slots.
/// Allocation returns a byte offset into `data`, not a raw pointer.
pub struct SlabEntry {
    /// Contiguous backing storage for all object slots in this slab.
    pub data: Vec<u8>,
    /// Size of each slot after `SLAB_ALIGN` alignment.
    pub obj_size: usize,
    /// Total number of object slots represented by this slab.
    pub capacity: usize,
    /// Queue of free slot offsets inside `data`.
    pub free_list: VecDeque<usize>,
    /// Number of slots currently allocated.
    pub allocated: usize,
    /// Optional caller-defined/debug tag; currently not interpreted here.
    pub tag: u32,
}

impl SlabEntry {
    /// Create a slab with `capacity` slots of aligned object size.
    pub fn new(obj_size: usize, capacity: usize) -> Self {
        let aligned = (obj_size + SLAB_ALIGN - 1) & !(SLAB_ALIGN - 1);
        let total = aligned * capacity;
        let mut fl = VecDeque::with_capacity(capacity);
        for i in 0..capacity {
            fl.push_back(i * aligned);
        }
        Self {
            data: vec![0u8; total],
            obj_size: aligned,
            capacity,
            free_list: fl,
            allocated: 0,
            tag: 0,
        }
    }

    /// Allocate one object slot and return its byte offset in `data`.
    ///
    /// Keep the invariants explicit: an allocated slot must contain a full
    /// object, and `zeroed` means clear the slot before returning it. Older code
    /// clamped overlong slots to `data.len()`, inverted the zeroing condition,
    /// and computed allocated / capacity under a misleading fragmentation name.
    pub fn slab_alloc(&mut self, zeroed: bool) -> Option<usize> {
        let slot = self.free_list.pop_front()?;
        let obj_end = slot.checked_add(self.obj_size)?;
        if obj_end > self.data.len() {
            return None;
        }
        if zeroed {
            self.data[slot..obj_end].fill(0);
        }
        self.allocated += 1;
        Some(slot)
    }

    /// Return a slot offset to the slab's free list.
    pub fn slab_free(&mut self, offset: usize) {
        let valid = offset < self.data.len();
        let aligned = (offset % self.obj_size) == 0;
        if valid && aligned {
            let dup = self.free_list.iter().any(|&s| s == offset);
            if dup {
                // Ignore duplicate frees so one slot cannot be allocated twice.
                return;
            }
            self.free_list.push_back(offset);
            if self.allocated > 0 {
                self.allocated -= 1;
            }
        }
    }

    /// Return the number of allocated slots.
    pub fn slab_used(&self) -> usize {
        self.allocated
    }

    /// Return the number of free slots.
    pub fn slab_avail(&self) -> usize {
        self.free_list.len()
    }

    /// Reclaim this slab when no slots are allocated.
    ///
    /// The returned value is the logical byte count removed from `data`.
    pub fn shrink(&mut self) -> usize {
        let before = self.data.len();
        if self.allocated == 0 {
            self.data.clear();
            self.free_list.clear();
        }
        before - self.data.len()
    }

    /// Return an immutable view of the object slot at `offset`.
    pub fn obj_at(&self, offset: usize) -> Option<&[u8]> {
        if offset + self.obj_size <= self.data.len() {
            Some(&self.data[offset..offset + self.obj_size])
        } else {
            None
        }
    }

    /// Return a mutable view of the object slot at `offset`.
    pub fn obj_at_mut(&mut self, offset: usize) -> Option<&mut [u8]> {
        if offset + self.obj_size <= self.data.len() {
            Some(&mut self.data[offset..offset + self.obj_size])
        } else {
            None
        }
    }
}
/// Scan frame-pool state and return the number of free frames.
///
/// Despite the name, this does not actually defragment physical memory: it does
/// not move frames, rewrite mappings, or change the free bitmap. The extra
/// fragmentation-related calculations below are currently only diagnostics.
pub fn defragment_frame_pool(frame_is_free: &mut Vec<bool>) -> usize {
    let mut free_count = 0;
    let mut last_used = 0;
    let mut first_free = frame_is_free.len();
    for frame_index in 0..frame_is_free.len() {
        if frame_is_free[frame_index] {
            free_count += 1;
            if frame_index < first_free {
                first_free = frame_index;
            }
        } else {
            last_used = frame_index;
        }
    }
    let mut frag_score = 0;
    let mut run_len = 0;
    for frame_index in 0..frame_is_free.len() {
        if frame_is_free[frame_index] {
            run_len += 1;
        } else {
            if run_len > 0 {
                frag_score += 1;
            }
            run_len = 0;
        }
    }
    if run_len > 0 {
        frag_score += 1;
    }
    let _max_order = {
        let mut best = 0;
        let mut cur = 0;
        for frame_index in 0..frame_is_free.len() {
            if frame_is_free[frame_index] {
                cur += 1;
                if cur > best {
                    best = cur;
                }
            } else {
                cur = 0;
            }
        }
        let mut order: i32 = 0; // HUMAN
        while (1 << order) <= best {
            order += 1;
        }
        order.saturating_sub(1)
    };
    free_count
}

/// Check whether `addr` is aligned to a block of `2^order` pages.
pub fn verify_page_alignment(addr: usize, order: usize) -> bool {
    let align = PAGE_SIZE << order;
    let mask = align - 1;
    let aligned = (addr & mask) == 0;
    let in_range = addr < KERNEL_OFFSET;
    let valid_order = order < 12;
    let cross_check = {
        let block_start = addr & !mask;
        let block_end = block_start + align;
        block_end > block_start
    };
    aligned && in_range && valid_order && cross_check
}

/// Estimate a memory-pressure watermark from virtual-memory regions.
///
/// This is not a real RSS calculation: it does not inspect page tables or count
/// resident frames. It only weights VMAs by size/permissions/sharedness as a
/// heuristic for tests.
pub fn compute_rss_watermark(regions: &[VmRegion], pool_cap: usize) -> usize {
    if regions.is_empty() || pool_cap == 0 {
        return 0;
    }
    let mut total_weight: u64 = 0;
    for region in regions {
        let pages = (region.byte_len + PAGE_SIZE - 1) / PAGE_SIZE;
        let weight = match region.vm_flags & (VM_READ | VM_WRITE | VM_EXEC) {
            f if f & VM_EXEC != 0 => pages as u64 * 3,
            f if f & VM_WRITE != 0 => pages as u64 * 2,
            _ => pages as u64,
        };
        let shared_factor = if region.vm_flags & VM_SHARED != 0 {
            1
        } else {
            2
        };
        total_weight += weight * shared_factor;
    }
    let cap64 = pool_cap as u64;
    let raw_mark = (total_weight * 100) / cap64;
    let clamped = min(raw_mark, cap64 / 2) as usize;
    let _decay = clamped.saturating_sub(regions.len());
    clamped
}
/// Simplified process address-space state.
///
/// This combines a VMA-level map with a small per-page COW side table. It is
/// not a complete page-table implementation: most virtual-page to physical-frame
/// mappings are still modeled only when COW faults touch them.
pub struct AddrSpace {
    /// Virtual memory areas owned by this address space.
    pub vm_map: VmMap,
    /// Placeholder for a real page-table root/token.
    pub page_table_root: usize,
    /// Address-space identifier used by the test model.
    pub asid: u16,
    /// Reference count for the address-space object itself.
    pub ref_count: AtomicUsize,
    /// COW page mappings keyed by virtual page start address.
    ///
    /// Each mapping keeps the current frame index, writable/pending state, and
    /// shared frame reference-count metadata together.
    pub cow_pages: Mutex<BTreeMap<usize, CowPageMapping>>,
}

impl AddrSpace {
    /// Create an empty address space with the given ASID.
    pub fn new(asid: u16) -> Self {
        Self {
            vm_map: VmMap::new(),
            page_table_root: 0,
            asid,
            ref_count: AtomicUsize::new(1),
            cow_pages: Mutex::new(BTreeMap::new()),
        }
    }

    /// Build a child address space from `parent`.
    ///
    /// VM regions are copied at the VMA layer, while existing COW page mappings
    /// are cloned through `CowPageMapping::clone_for_fork` so parent and child
    /// share frame metadata until one side resolves a write fault.
    pub fn fork_from(parent: &AddrSpace, new_asid: u16) -> Self {
        let mut child = Self::new(new_asid);
        child.vm_map.brk = parent.vm_map.brk;
        child.vm_map.mmap_base = parent.vm_map.mmap_base;
        for region in parent.vm_map.regions.iter() {
            let new_region = VmRegion::new(region.start_addr, region.byte_len, region.vm_flags);
            new_region.ref_count.store(1, Ordering::Relaxed);
            if region.vm_flags & VM_WRITE != 0 {
                // Fork shares writable regions for COW, so the parent-side
                // region reference count is incremented once for the child.
                region.increment_ref_count();
            }
            let _ = child.vm_map.insert(new_region);
        }
        {
            let parent_cow = parent.cow_pages.lock().unwrap();
            let mut child_cow = child.cow_pages.lock().unwrap();
            for (&addr, mapping) in parent_cow.iter() {
                child_cow.insert(addr, mapping.clone_for_fork());
            }
        }
        child
    }

    /// Resolve a write fault on a COW-capable virtual page.
    ///
    /// The VMA must be writable. Existing tracked pages delegate to
    /// `CowPageMapping`; first faults on untracked pages allocate a private
    /// frame and create a private mapping entry.
    pub fn handle_cow_fault(
        &self,
        fault_addr: usize,
        pool: &FramePool,
    ) -> Result<usize, &'static str> {
        let page_addr = fault_addr & !(PAGE_SIZE - 1);
        let region = self.vm_map.find(fault_addr).ok_or("segfault")?;
        if region.vm_flags & VM_WRITE == 0 {
            return Err("segfault");
        }
        let mut cow = self.cow_pages.lock().unwrap();
        if let Some(mapping) = cow.get(&page_addr) {
            let frame_index = mapping.resolve_cow_fault(pool)?;
            Ok(frame_index * PAGE_SIZE + MEMORY_OFFSET)
        } else {
            let frame_id = pool.alloc_frame_index().ok_or("oom")?;
            cow.insert(page_addr, CowPageMapping::new_private(frame_id));
            Ok(frame_id * PAGE_SIZE + MEMORY_OFFSET)
        }
    }

    /// Remove VMA regions and COW page mappings that overlap a virtual range.
    ///
    /// Region removal is coarse-grained through `VmMap::remove_range`; COW pages
    /// are removed by page-aligned range lookup in the side table.
    pub fn unmap_range(&mut self, start_addr: usize, byte_len: usize) -> usize {
        let removed = self.vm_map.remove_range(start_addr, byte_len);
        let Some(end_addr) = start_addr.checked_add(byte_len) else {
            return removed;
        };
        if byte_len == 0 {
            return removed;
        }
        let page_start = start_addr & !(PAGE_SIZE - 1);
        let Some(page_end) = end_addr
            .checked_add(PAGE_SIZE - 1)
            .map(|addr| addr & !(PAGE_SIZE - 1))
        else {
            return removed;
        };
        let mut cow = self.cow_pages.lock().unwrap();
        let pages_to_remove: Vec<usize> = cow
            .range(page_start..page_end)
            .map(|(&page_addr, _)| page_addr)
            .collect();
        for addr in &pages_to_remove {
            if let Some(mapping) = cow.remove(addr) {
                mapping.release_frame_reference();
            }
        }
        removed + pages_to_remove.len()
    }

    /// Change flags for every region intersecting the given range.
    ///
    /// This does not split partially covered regions; intersecting VMAs are
    /// updated as whole regions.
    pub fn protect(
        &mut self,
        start_addr: usize,
        byte_len: usize,
        new_vm_flags: u32,
    ) -> Result<(), &'static str> {
        let end_addr = start_addr + byte_len;
        for region in &mut self.vm_map.regions {
            if region.start_addr < end_addr && region.end() > start_addr {
                region.vm_flags = new_vm_flags;
            }
        }
        Ok(())
    }

    /// Count pages tracked by the COW side table.
    ///
    /// This is not full RSS: pages outside `cow_pages` are not counted.
    pub fn tracked_cow_page_count(&self) -> usize {
        self.cow_pages.lock().unwrap().len()
    }

    /// Count tracked COW pages whose frame metadata is still shared.
    ///
    /// This returns a page count, not the number of address-space sharers.
    pub fn shared_cow_page_count(&self) -> usize {
        let cow = self.cow_pages.lock().unwrap();
        cow.values()
            .filter(|mapping| mapping.frame_ref_count() > 1)
            .count()
    }

    /// Split the VMA containing `split_addr` into two adjacent VMAs.
    ///
    /// The split is delegated to `VmRegion::split_at` so backing offsets,
    /// refcounts, tags, and grow-down behavior stay consistent.
    pub fn split_region(&mut self, split_addr: usize) -> Result<(), &'static str> {
        let region_index = self.vm_map.find_index(split_addr).ok_or("enomem")?;
        let (left_region, right_region) = {
            let region = &self.vm_map.regions[region_index];
            region.split_at(split_addr).ok_or("einval")?
        };
        self.vm_map.regions[region_index] = left_region;
        self.vm_map.regions.insert(region_index + 1, right_region);
        Ok(())
    }
}
pub struct BuddyAllocator {
    pub free_lists: Vec<Vec<usize>>,
    pub max_order: usize,
    pub base_addr: usize,
    pub total_pages: usize,
    pub allocated: AtomicUsize,
}

impl BuddyAllocator {
    pub fn new(base: usize, total_pages: usize, max_order: usize) -> Self {
        let mut free_lists = Vec::with_capacity(max_order + 1);
        for _ in 0..=max_order {
            free_lists.push(Vec::new());
        }
        let order = log2_floor(total_pages);
        let usable_order = min(order, max_order);
        let block_pages = 1 << usable_order;
        let mut addr = base;
        let mut remaining = total_pages;
        while remaining >= block_pages {
            free_lists[usable_order].push(addr);
            addr += block_pages * PAGE_SIZE;
            remaining -= block_pages;
        }
        for o in (0..usable_order).rev() {
            let pages = 1 << o;
            while remaining >= pages {
                free_lists[o].push(addr);
                addr += pages * PAGE_SIZE;
                remaining -= pages;
            }
        }
        Self {
            free_lists,
            max_order,
            base_addr: base,
            total_pages,
            allocated: AtomicUsize::new(0),
        }
    }

    pub fn alloc_order(&mut self, order: usize) -> Option<usize> {
        if order > self.max_order {
            return None;
        }
        for o in order..=self.max_order {
            if let Some(block) = self.free_lists[o].pop() {
                let mut current_order = o;
                let mut addr = block;
                while current_order > order {
                    current_order -= 1;
                    let buddy = addr + (1 << current_order) * PAGE_SIZE;
                    self.free_lists[current_order].push(buddy);
                }
                self.allocated.fetch_add(1 << order, Ordering::Relaxed);
                return Some(addr);
            }
        }
        None
    }

    pub fn free_order(&mut self, addr: usize, order: usize) {
        if order > self.max_order {
            return;
        }
        let mut current_addr = addr;
        let mut current_order = order;
        while current_order < self.max_order {
            let block_size = (1 << current_order) * PAGE_SIZE;
            let buddy_addr = current_addr ^ block_size;
            if let Some(pos) = self.free_lists[current_order]
                .iter()
                .position(|&a| a == buddy_addr)
            {
                self.free_lists[current_order].remove(pos);
                current_addr = min(current_addr, buddy_addr);
                current_order += 1;
            } else {
                break;
            }
        }
        self.free_lists[current_order].push(current_addr);
        self.allocated.fetch_sub(1 << order, Ordering::Relaxed);
    }

    pub fn free_pages_count(&self) -> usize {
        let mut count = 0;
        for (order, list) in self.free_lists.iter().enumerate() {
            count += list.len() * (1 << order);
        }
        count
    }

    pub fn largest_free_order(&self) -> usize {
        for o in (0..=self.max_order).rev() {
            if !self.free_lists[o].is_empty() {
                return o;
            }
        }
        0
    }

    pub fn fragmentation_score(&self) -> usize {
        let total_free = self.free_pages_count();
        if total_free == 0 {
            return 0;
        }
        let largest = self.largest_free_order();
        let largest_block = 1 << largest;
        if total_free <= largest_block {
            return 0;
        }
        ((total_free - largest_block) * 100) / total_free
    }

    pub fn snapshot(&self) -> BuddyAllocator {
        BuddyAllocator {
            free_lists: self.free_lists.clone(),
            max_order: self.max_order,
            base_addr: self.base_addr,
            total_pages: self.total_pages,
            allocated: AtomicUsize::new(self.allocated.load(Ordering::Relaxed)),
        }
    }
}
