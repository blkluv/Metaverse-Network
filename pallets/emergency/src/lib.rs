// This file is part of Metaverse.Network & Bit.Country.

// Copyright (C) 2020-2022 Metaverse.Network & Bit.Country .
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::unused_unit)]

use frame_support::{
	pallet_prelude::*,
	traits::{CallMetadata, Contains, GetCallMetadata, PalletInfoAccess},
	transactional,
};
use frame_system::pallet_prelude::*;
use sp_runtime::DispatchResult;
use sp_std::{prelude::*, vec::Vec};

pub use module::*;
pub use weights::WeightInfo;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

pub mod weights;

#[frame_support::pallet]
pub mod module {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The origin which may set filter.
		type EmergencyOrigin: EnsureOrigin<Self::RuntimeOrigin>;
		/// The base call filter to be used in normal operating mode
		type NormalCallFilter: Contains<Self::RuntimeCall>;
		/// The base call filter to be used when we are in the middle of migrations
		type MaintenanceCallFilter: Contains<Self::RuntimeCall>;
		/// Extrinsics' weights
		type WeightInfo: WeightInfo;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Can not stop emergency call
		CannotStopEmergencyCall,
		/// invalid character encoding
		InvalidPalletAndFunction,
		/// The chain cannot enter maintenance mode because it is already in maintenance mode
		AlreadyInMaintenanceMode,
		/// The chain cannot resume normal operation because it is not in maintenance mode
		NotInMaintenanceMode,
	}

	#[pallet::event]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event<T: Config> {
		/// Stopped transaction
		EmergencyStopped {
			pallet_name_bytes: Vec<u8>,
			function_name_bytes: Vec<u8>,
		},
		/// Unstopped transaction
		EmergencyUnStopped {
			pallet_name_bytes: Vec<u8>,
			function_name_bytes: Vec<u8>,
		},
		/// Chain is enter maintenance mode
		MaintenanceModeStarted,
		/// Chain is exit maintenance mode and enter normal operation
		MaintenanceModeEnded,
	}

	/// The paused transaction map
	///
	/// map (PalletNameBytes, FunctionNameBytes) => Option<()>
	#[pallet::storage]
	#[pallet::getter(fn emergency_stopped_pallets)]
	pub type EmergencyStoppedPallets<T: Config> = StorageMap<_, Twox64Concat, (Vec<u8>, Vec<u8>), (), OptionQuery>;

	#[pallet::storage]
	#[pallet::getter(fn maintenance_mode)]
	/// If the chain is in maintenance mode
	type MaintenanceMode<T: Config> = StorageValue<_, bool, ValueQuery>;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(T::WeightInfo::emergency_stop())]
		#[transactional]
		pub fn emergency_stop(origin: OriginFor<T>, pallet_name: Vec<u8>, function_name: Vec<u8>) -> DispatchResult {
			T::EmergencyOrigin::ensure_origin(origin)?;

			// not allowed to pause calls of this pallet to ensure safe
			let pallet_name_string =
				sp_std::str::from_utf8(&pallet_name).map_err(|_| Error::<T>::InvalidPalletAndFunction)?;
			ensure!(
				pallet_name_string != <Self as PalletInfoAccess>::name(),
				Error::<T>::CannotStopEmergencyCall
			);

			EmergencyStoppedPallets::<T>::mutate_exists((pallet_name.clone(), function_name.clone()), |maybe_paused| {
				if maybe_paused.is_none() {
					*maybe_paused = Some(());
					Self::deposit_event(Event::EmergencyStopped {
						pallet_name_bytes: pallet_name,
						function_name_bytes: function_name,
					});
				}
			});
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::emergency_unstop())]
		#[transactional]
		pub fn emergency_unstop(origin: OriginFor<T>, pallet_name: Vec<u8>, function_name: Vec<u8>) -> DispatchResult {
			T::EmergencyOrigin::ensure_origin(origin)?;
			if EmergencyStoppedPallets::<T>::take((&pallet_name, &function_name)).is_some() {
				Self::deposit_event(Event::EmergencyUnStopped {
					pallet_name_bytes: pallet_name,
					function_name_bytes: function_name,
				});
			};
			Ok(())
		}

		#[pallet::weight(T::DbWeight::get().read + 2 * T::DbWeight::get().write)]
		#[transactional]
		pub fn start_maintenance_mode(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			// Ensure Origin
			T::EmergencyOrigin::ensure_origin(origin)?;

			// Ensure we're not aleady in maintenance mode.
			ensure!(!MaintenanceMode::<T>::get(), Error::<T>::AlreadyInMaintenanceMode);

			MaintenanceMode::<T>::put(true);

			// Event
			Self::deposit_event(Event::MaintenanceModeStarted);

			Ok(().into())
		}

		#[pallet::weight(T::DbWeight::get().read + 2 * T::DbWeight::get().write)]
		#[transactional]
		pub fn exit_maintenance_mode(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			// Ensure Origin
			T::EmergencyOrigin::ensure_origin(origin)?;

			// Ensure we're started the maintenance mode.
			ensure!(MaintenanceMode::<T>::get(), Error::<T>::NotInMaintenanceMode);

			MaintenanceMode::<T>::put(false);

			Self::deposit_event(Event::MaintenanceModeEnded);

			Ok(().into())
		}
	}

	impl<T: Config> Contains<T::RuntimeCall> for Pallet<T> {
		fn contains(call: &T::RuntimeCall) -> bool {
			if MaintenanceMode::<T>::get() {
				T::MaintenanceCallFilter::contains(call)
			} else {
				T::NormalCallFilter::contains(call)
			}
		}
	}
}

pub struct EmergencyStoppedFilter<T>(sp_std::marker::PhantomData<T>);

impl<T: Config> Contains<T::RuntimeCall> for EmergencyStoppedFilter<T>
where
	<T as frame_system::Config>::RuntimeCall: GetCallMetadata,
{
	fn contains(call: &T::RuntimeCall) -> bool {
		let CallMetadata {
			function_name,
			pallet_name,
		} = call.get_call_metadata();

		EmergencyStoppedPallets::<T>::contains_key((pallet_name.as_bytes(), function_name.as_bytes()))
	}
}
