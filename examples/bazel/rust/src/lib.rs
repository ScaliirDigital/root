use uuid::Uuid;

pub fn message() -> String {
    format!("Hello from Rust & Bazel!\nUUID: {}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::message;

    #[test]
    fn message_contains_expected_prefix() {
        let value = message();

        assert!(value.starts_with("Hello from Rust & Bazel!\nUUID: "));
    }
}
