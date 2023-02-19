fn main() {
    #[allow(clippy::expect_used)]
    tonic_build::compile_protos("protos/server-side-streaming.proto")
        .expect("Failed to compile `server-side-streaming.proto.proto` file");
    #[allow(clippy::expect_used)]
    tonic_build::compile_protos("protos/client-side-streaming.proto")
        .expect("Failed to compile `client-side-streaming.proto.proto` file");
}
