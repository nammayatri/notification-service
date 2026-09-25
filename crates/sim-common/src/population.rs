/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use notification_service::{common::utils::hash_uuid, redis::keys::notification_client_key};
use uuid::Uuid;

pub fn client_id(index: u64) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("sim-{index}").as_bytes()).to_string()
}

pub fn shard(client_id: &str, max_shards: u64) -> u64 {
    (hash_uuid(client_id) % max_shards as u128) as u64
}

pub fn stream_key(client_id: &str, shard: u64) -> String {
    notification_client_key(client_id, &shard)
}

pub fn log_population_fingerprint(role: &str, index_start: u64, count: u64, max_shards: u64) {
    let samples = count.min(3);
    for offset in 0..samples {
        let index = index_start + offset;
        let id = client_id(index);
        let shard = shard(&id, max_shards);
        tracing::info!(
            "[{}] fingerprint index={} client_id={} shard={} key={}",
            role,
            index,
            id,
            shard,
            stream_key(&id, shard)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_stable_across_binaries() {
        assert_eq!(client_id(0), "59f9c690-caa0-5595-aa99-8d80f824e223");
        assert_eq!(client_id(1), "c99406c6-c950-5eed-84b9-2d75d81362a5");
        assert_eq!(client_id(19), "ac003980-36f1-5fac-bdd7-5b53b50b8dab");
    }

    #[test]
    fn shards_and_keys_match_the_service_formula() {
        let id = client_id(0);
        assert_eq!(shard(&id, 128), 56);
        assert_eq!(
            stream_key(&id, 56),
            "N59f9c690-caa0-5595-aa99-8d80f824e223{56}"
        );
    }

    #[test]
    fn replica_splits_cover_the_population_exactly_once() {
        let whole: Vec<String> = (0..200).map(client_id).collect();
        let split: Vec<String> = (0..100).chain(100..200).map(client_id).collect();
        assert_eq!(whole, split);
    }
}
