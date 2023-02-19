pub mod chat {
    tonic::include_proto!("chat");

    impl User {
        pub fn new(id: &str, name: &str) -> Self {
            Self {
                id: id.to_owned(),
                name: name.to_owned(),
            }
        }
    }
}
