pub mod server_side_streaming {
    tonic::include_proto!("server_side_streaming");

    impl User {
        pub fn new(id: &str, name: &str) -> Self {
            Self {
                id: id.to_owned(),
                name: name.to_owned(),
            }
        }
    }
}

pub mod client_side_streaming {
    tonic::include_proto!("client_side_streaming");
}
