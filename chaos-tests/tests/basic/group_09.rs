use chaos_tests::*;

#[test]
fn basic_save_restore_context() {
    let mut registers = [0u64; N_REGS];
    registers[0] = 0xAA;
    registers[1] = 0xBB;
    registers[2] = 0xCC;
    let trap_frame = SimTrapFrame::from_registers(&registers);
    let restored_registers = trap_frame.to_registers();
    assert_eq!(restored_registers[0], 0xAA);
}

#[test]
fn basic_interrupt_mask_set() {
    let trap_controller = TrapController::new();
    trap_controller.configure_vector_masks(0xFF, 0x00);
    assert_eq!(trap_controller.hardware_vector_mask(), 0x00);
}

#[test]
fn basic_page_fault_in_process_context() {
    let trap_controller = TrapController::new();
    let result = trap_controller.handle_page_fault(0x1000);
    assert!(result.is_ok());
}
