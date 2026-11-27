use std::process::Command;

pub fn assert_sealed_refusal(program: &str, args: &[&str]) {
    // seal-allow: this test executes the consumer and asserts its guard rejects sealed paths.
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("run sealed-path consumer");
    assert_eq!(output.status.code(), Some(2), "{args:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("sealed"));
}

pub fn assert_usage_refusal(program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .output()
        .expect("run usage refusal");
    assert_eq!(output.status.code(), Some(2));
}
