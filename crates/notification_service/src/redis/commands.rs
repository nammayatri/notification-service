/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use super::keys::*;
use crate::common::types::*;
use anyhow::Result;
use shared::redis::types::RedisConnectionPool;

pub async fn set_client_id(
    redis_pool: &RedisConnectionPool,
    auth_token_expiry: &u32,
    token: &Token,
    ClientId(client_id): &ClientId,
) -> Result<()> {
    redis_pool
        .set_key_as_str(&set_client_id_key(token), client_id, *auth_token_expiry)
        .await?;
    Ok(())
}

pub async fn get_client_id(
    redis_pool: &RedisConnectionPool,
    token: &Token,
) -> Result<Option<ClientId>> {
    Ok(redis_pool
        .get_key_as_str(&set_client_id_key(token))
        .await?
        .map(ClientId))
}
