/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::unwrap_used)]

use crate::redis::types::*;
use anyhow::{anyhow, Result};
use fred::{
    interfaces::KeysInterface,
    types::{Expiration, RedisValue},
};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;

impl RedisConnectionPool {
    pub async fn set_key<V>(&self, key: &str, value: V, expiry: u32) -> Result<()>
    where
        V: Serialize + Send + Sync,
    {
        let serialized_value = serde_json::to_string(&value)?;

        let redis_value: RedisValue = serialized_value.into();

        self.pool
            .set(
                key,
                redis_value,
                Some(Expiration::EX(expiry.into())),
                None,
                false,
            )
            .await?;

        Ok(())
    }

    pub async fn setnx_with_expiry<V>(&self, key: &str, value: V, expiry: i64) -> Result<bool>
    where
        V: TryInto<RedisValue> + Debug + Send + Sync,
        V::Error: Into<fred::error::RedisError> + Send + Sync,
    {
        let output = self.pool.msetnx::<RedisValue, _>((key, value)).await?;
        self.pool.expire::<(), &str>(key, expiry).await?;

        match output {
            RedisValue::Integer(1) => Ok(true),
            RedisValue::Integer(0) => Ok(false),
            case => Err(anyhow!("Unexpected RedisValue encountered : {:?}", case)),
        }
    }

    pub async fn set_expiry(&self, key: &str, seconds: i64) -> Result<()> {
        self.pool.expire(key, seconds).await?;
        Ok(())
    }

    pub async fn get_key<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let output: RedisValue = self.pool.get(key).await?;

        match output {
            RedisValue::String(val) => Ok(serde_json::from_str(&val).map(Some)?),
            RedisValue::Null => Ok(None),
            case => Err(anyhow!("Unexpected RedisValue encountered : {:?}", case)),
        }
    }

    pub async fn delete_key(&self, key: &str) -> Result<()> {
        self.pool.del(key).await?;
        Ok(())
    }
}
