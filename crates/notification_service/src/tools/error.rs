/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use tonic::Status;

#[macros::add_error]
pub enum AppError {
    InternalError(String),
    InvalidRequest(String),
    Timeout,
    DriverAppAuthFailed,
    DriverAppUnauthorized,
}

impl From<AppError> for Status {
    fn from(app_error: AppError) -> Self {
        match app_error {
            AppError::InternalError(_) => Status::internal("INTERNAL_ERROR"),
            AppError::InvalidRequest(_) => Status::invalid_argument("INVALID_REQUEST"),
            AppError::Timeout => Status::internal("TIMEOUT"),
            AppError::DriverAppAuthFailed => Status::invalid_argument("AUTH_FAILED"),
            AppError::DriverAppUnauthorized => Status::unauthenticated("UNAUTHORIZED"),
        }
    }
}
