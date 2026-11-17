use super::hyos_mount_timeout_ms;

#[test]
fn mount_budget_covers_recovery_and_scales_with_the_actual_roots() {
    assert_eq!(hyos_mount_timeout_ms(0, 6000), Some(19000));
    assert_eq!(hyos_mount_timeout_ms(1, 6000), Some(27000));
    assert_eq!(hyos_mount_timeout_ms(3, 6000), Some(43000));
    assert_eq!(hyos_mount_timeout_ms(usize::MAX, 6000), None);
    assert_eq!(hyos_mount_timeout_ms(1, u64::MAX), None);
}
