fn main() {
    #[allow(clippy::expect_used)]
    tonic_build::compile_protos("protos/chat.proto").expect("Failed to compile `chat.proto` file");
}
