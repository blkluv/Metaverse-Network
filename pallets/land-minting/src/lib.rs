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

use frame_support::pallet_prelude::*;
use frame_support::{
	dispatch::DispatchResult,
	ensure, log,
	traits::{Currency, ExistenceRequirement, Get},
	transactional, PalletId,
};
use frame_system::pallet_prelude::*;
use frame_system::{ensure_root, ensure_signed};
use orml_traits::MultiCurrency;
use scale_info::TypeInfo;
use sp_runtime::traits::CheckedSub;
use sp_runtime::{
	traits::{AccountIdConversion, Convert, One, Saturating, Zero},
	ArithmeticError, DispatchError, Perbill, SaturatedConversion,
};
use sp_std::vec::Vec;

use auction_manager::{Auction, CheckAuctionItemHandler};
use core_primitives::*;
pub use pallet::*;
use primitives::estate::EstateInfo;
use primitives::{
	estate::{Estate, LandUnitStatus, LeaseContract, OwnerId},
	Attributes, ClassId, EstateId, FungibleTokenId, ItemId, MetaverseId, NftMetadata, StakingRound, TokenId,
	UndeployedLandBlock, UndeployedLandBlockId, UndeployedLandBlockType,
};
pub use weights::WeightInfo;

pub type QueueId = u32;
//#[cfg(feature = "runtime-benchmarks")]
//pub mod benchmarking;

#[cfg(test)]
mod mock;
mod utils;

#[cfg(test)]
mod tests;

pub mod weights;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::traits::{Currency, Imbalance, ReservableCurrency};
	use orml_traits::{MultiCurrency, MultiReservableCurrency};
	use sp_core::U256;
	use sp_runtime::traits::{CheckedAdd, CheckedSub, Zero};
	use sp_runtime::Permill;

	use primitives::estate::EstateInfo;
	use primitives::staking::{Bond, RoundInfo, StakeSnapshot};
	use primitives::{AccountId, Balance, CurrencyId, PoolId, RoundIndex, StakingRound, UndeployedLandBlockId};

	use crate::utils::PoolInfo;

	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(PhantomData<T>);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Because this pallet emits events, it depends on the runtime's definition of an event.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Land treasury source
		#[pallet::constant]
		type LandTreasury: Get<PalletId>;

		/// Source of metaverse info
		type MetaverseInfoSource: MetaverseTrait<Self::AccountId>;

		/// Currency type
		type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;
		/// Multi currencies type that handles different currency type in auction
		type MultiCurrency: MultiReservableCurrency<Self::AccountId, CurrencyId = FungibleTokenId, Balance = Balance>;

		/// Weight implementation for estate extrinsics
		type WeightInfo: WeightInfo;

		/// Minimum staking balance
		#[pallet::constant]
		type MinimumStake: Get<BalanceOf<Self>>;

		/// Delay of staking reward payment (in number of rounds)
		#[pallet::constant]
		type RewardPaymentDelay: Get<u32>;

		/// NFT trait required for land and estate tokenization
		type NFTTokenizationSource: NFTTrait<Self::AccountId, BalanceOf<Self>, ClassId = ClassId, TokenId = TokenId>;

		/// Default max bound for each metaverse mapping system, this could change through proposal
		type DefaultMaxBound: Get<(i32, i32)>;

		/// Network fee charged when depositing or redeeming
		#[pallet::constant]
		type NetworkFee: Get<BalanceOf<Self>>;

		/// Storage deposit free charged when saving data into the blockchain.
		/// The fee will be unreserved after the storage is freed.
		#[pallet::constant]
		type StorageDepositFee: Get<BalanceOf<Self>>;

		/// Allows converting block numbers into balance
		type BlockNumberToBalance: Convert<Self::BlockNumber, BalanceOf<Self>>;

		#[pallet::constant]
		type PoolAccount: Get<PalletId>;

		#[pallet::constant]
		type MaximumQueue: Get<u32>;
	}

	pub type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
	pub type CurrencyIdOf<T> =
		<<T as Config>::MultiCurrency as MultiCurrency<<T as frame_system::Config>::AccountId>>::CurrencyId;

	#[pallet::storage]
	#[pallet::getter(fn next_class_id)]
	pub type NextPoolId<T: Config> = StorageValue<_, PoolId, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn fees)]
	pub type Fees<T: Config> = StorageValue<_, (Permill, Permill), ValueQuery>;

	/// Keep track of Pool detail
	#[pallet::storage]
	#[pallet::getter(fn pool)]
	pub type Pool<T: Config> = StorageMap<_, Twox64Concat, PoolId, PoolInfo<CurrencyIdOf<T>, T::AccountId>, ValueQuery>;

	/// Pool ledger that keeps track of Pool id and balance of base currency
	#[pallet::storage]
	#[pallet::getter(fn pool_ledger)]
	pub type PoolLedger<T: Config> = StorageMap<_, Twox64Concat, PoolId, BalanceOf<T>, ValueQuery>;

	/// Network ledger that keep track of all staking across all pools
	#[pallet::storage]
	#[pallet::getter(fn network_ledger)]
	pub type NetworkLedger<T: Config> = StorageMap<_, Twox64Concat, CurrencyIdOf<T>, BalanceOf<T>, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn minimum_redeem)]
	pub type MinimumRedeem<T: Config> = StorageMap<_, Twox64Concat, CurrencyIdOf<T>, BalanceOf<T>, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn network_redeem_requests)]
	pub type NetworkRedeemQueue<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		StakingRound,
		Blake2_128Concat,
		CurrencyIdOf<T>,
		(BalanceOf<T>, BoundedVec<QueueId, T::MaximumQueue>, CurrencyIdOf<T>),
		OptionQuery,
	>;

	#[pallet::storage]
	#[pallet::getter(fn user_unlock_request)]
	pub type UserUnlockRequest<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		CurrencyIdOf<T>,
		Blake2_128Concat,
		QueueId,
		(T::AccountId, BalanceOf<T>, StakingRound),
		OptionQuery,
	>;

	#[pallet::storage]
	#[pallet::getter(fn unlock_duration)]
	pub type UnlockDuration<T: Config> = StorageMap<_, Twox64Concat, CurrencyIdOf<T>, StakingRound>;

	#[pallet::storage]
	#[pallet::getter(fn current_staking_round)]
	pub type CurrentStakingRound<T: Config> = StorageMap<_, Twox64Concat, CurrencyIdOf<T>, StakingRound>;

	#[pallet::storage]
	#[pallet::getter(fn queue_next_id)]
	pub type QueueNextId<T: Config> = StorageMap<_, Twox64Concat, CurrencyIdOf<T>, u32, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub (crate) fn deposit_event)]
	pub enum Event<T: Config> {
		/// New staking round started [Starting Block, Round, Total Land Unit]
		NewRound(T::BlockNumber, RoundIndex, u64),
		/// New pool created
		PoolCreated(T::AccountId, PoolId, CurrencyIdOf<T>),
		/// Deposited
		Deposited(T::AccountId, PoolId, BalanceOf<T>),
		/// Redeemed
		Redeemed(T::AccountId, PoolId, BalanceOf<T>),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// No permission
		NoPermission,
		/// Currency is not supported
		CurrencyIsNotSupported,
		/// No available next pool id
		NoAvailablePoolId,
		/// Pool doesn't exists
		PoolDoesNotExist,
		/// Overflow
		Overflow,
		/// Below minimum redemption
		BelowMinimumRedeem,
		/// No current staking round
		NoCurrentStakingRound,
		/// Unexpected
		Unexpected,
		/// Too many redeems
		TooManyRedeems,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(T::WeightInfo::mint_land())]
		pub fn create_pool(
			origin: OriginFor<T>,
			currency_id: CurrencyIdOf<T>,
			max_nft_reward: u32,
			commission: Permill,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			// Ensure currency_id supported
			ensure!(
				currency_id == FungibleTokenId::NativeToken(0) || currency_id == FungibleTokenId::NativeToken(1),
				Error::<T>::CurrencyIsNotSupported
			);

			// TODO Check commission below threshold

			// Collect pool creation fee
			Self::collect_pool_creation_fee(&who)?;

			// Next pool id
			let next_pool_id = NextPoolId::<T>::try_mutate(|id| -> Result<PoolId, DispatchError> {
				let current_id = *id;
				*id = id.checked_add(1u32).ok_or(Error::<T>::NoAvailablePoolId)?;
				Ok(current_id)
			})?;

			let new_pool = PoolInfo {
				creator: who.clone(),
				commission: commission,
				currency_id: currency_id,
				max: max_nft_reward,
			};

			// Add tuple class_id, currency_id
			Pool::<T>::insert(next_pool_id, new_pool);

			// Emit event for pool creation
			Self::deposit_event(Event::PoolCreated(who, max_nft_reward, currency_id));
			Ok(().into())
		}

		#[pallet::weight(T::WeightInfo::mint_land())]
		pub fn deposit(origin: OriginFor<T>, pool_id: PoolId, amount: BalanceOf<T>) -> DispatchResult {
			// Ensure user is signed
			let who = ensure_signed(origin)?;
			// Check if pool exists
			let pool_instance = Pool::<T>::get(pool_id).ok_or(Error::<T>::PoolDoesNotExist)?;

			// Get currencyId from pool detail
			let currency_id = pool_instance.currency_id;

			// Get network ledger balance from currency id
			let network_ledger_balance = Self::network_ledger(currency_id);

			// Collect deposit fee for protocol
			// Assuming there's a function `collect_deposit_fee` that deducts a fee for deposits.
			let amount_after_fee = Self::collect_deposit_fee(&who, currency_id, amount)?;

			let v_currency_id = T::CurrencyIdManagement::convert_to_vcurrency(currency_id)
				.map_err(|_| Error::<T>::CurrencyIsNotSupported)?;
			// Calculate vAmount as receipt of amount locked. The formula based on vAmount = (amount * vAmount
			// total issuance)/network ledger balance
			let v_amount_total_issuance = T::MultiCurrency::total_issuance(v_currency_id);
			let v_amount = U256::from(amount_after_fee.saturated_into::<u128>())
				.saturating_mul(v_amount_total_issuance.saturated_into::<u128>().into())
				.checked_div(network_ledger_balance.saturated_into::<u128>().into())
				.ok_or(ArithmeticError::Overflow)?
				.as_u128()
				.saturated_into();

			// Deposit vAmount to user using T::MultiCurrency::deposit
			T::MultiCurrency::deposit(currency_id, &who, v_amount)?;

			// Update this specific pool ledger to keep track of pool balance
			PoolLedger::<T>::mutate(&pool_id, |pool| -> Result<(), Error<T>> {
				*pool = pool.checked_add(&amount_after_fee).ok_or(ArithmeticError::Overflow)?;
				Ok(())
			})?;

			NetworkLedger::<T>::mutate(&currency_id, |pool| -> Result<(), Error<T>> {
				*pool = pool.checked_add(&amount_after_fee).ok_or(ArithmeticError::Overflow)?;
				Ok(())
			})?;
			// Transfer amount to PoolAccount using T::MultiCurrency::transfer
			// Assuming `PoolAccount` is an associated type that represents the pool's account ID or a method to
			// get it.
			T::MultiCurrency::transfer(
				currency_id,
				&who,
				&T::PoolAccount::get().into_account_truncating(),
				amount,
			)?;

			// Emit deposit event
			Self::deposit_event(Event::Deposited(who, pool_id, amount));
			Ok(().into())
		}

		#[pallet::weight(T::WeightInfo::mint_land())]
		pub fn redeem(
			origin: OriginFor<T>,
			pool_id: PoolId,
			vcurrency_id: CurrencyIdOf<T>,
			vamount: BalanceOf<T>,
		) -> DispatchResult {
			// Ensure user is signed
			let who = ensure_signed(origin)?;
			ensure!(
				vamount >= MinimumRedeem::<T>::get(vcurrency_id),
				Error::<T>::BelowMinimumRedeem
			);

			let currency_id = T::CurrencyIdManagement::convert_to_currency(vcurrency_id)
				.map_err(|_| Error::<T>::NotSupportTokenType)?;

			// Check if pool exists
			let pool_instance = Pool::<T>::get(pool_id).ok_or(Error::<T>::PoolDoesNotExist)?;

			ensure!(
				currency_id == pool_instance.currency_id,
				Error::<T>::CurrencyIsNotSupported
			);

			// Get network ledger balance from currency id
			let network_ledger_balance = Self::network_ledger(currency_id);

			// Collect deposit fee for protocol
			// Assuming there's a function `collect_redeem_fee` that deducts a fee for deposits.
			let amount_after_fee = Self::collect_redeem_fee(&who, vcurrency_id, vamount)?;
			let vamount = vamount
				.checked_sub(&amount_after_fee)
				.ok_or(ArithmeticError::Overflow)?;
			// Calculate vAmount as receipt of amount locked. The formula based on vAmount = (amount * vAmount
			// total issuance)/network ledger balance
			let v_amount_total_issuance = T::MultiCurrency::total_issuance(vcurrency_id);
			let currency_amount = U256::from(vamount.saturated_into::<u128>())
				.saturating_mul(network_ledger_balance.saturated_into::<u128>().into())
				.checked_div(v_amount_total_issuance.saturated_into::<u128>().into())
				.ok_or(ArithmeticError::Overflow)?
				.as_u128()
				.saturated_into();

			// Check current staking era - only failed when there is no current staking era
			// Staking era get checked and updated every blocks
			match CurrentStakingRound::<T>::get(currency_id) {
				Some(staking_round) => {
					// Calculate the staking duration to be locked
					let new_staking_round = Self::calculate_next_staking_round(
						Self::unlock_duration(currency_id).ok_or(Error::<T>::UnlockDurationNotFound)?,
						staking_round,
					)?;
					// Burn currency
					T::MultiCurrency::withdraw(vcurrency_id, &who, vamount)?;

					// Update pool ledger
					PoolLedger::<T>::mutate(&pool_id, |pool| -> Result<(), Error<T>> {
						*pool = pool.checked_sub(&currency_amount).ok_or(ArithmeticError::Overflow)?;
						Ok(())
					})?;

					let next_queue_id = Self::queue_next_id(currency_id);
					UserUnlockRequest::<T>::insert(
						&currency_id,
						&next_queue_id,
						(&who, currency_amount, &new_staking_round),
					);

					if UserUnlockRequest::<T>::get(&who, &currency_id).is_some() {
						UserUnlockRequest::<T>::mutate(&who, &currency_id, |value| -> Result<(), Error<T>> {
							if let Some((total_locked, ledger_list)) = value {
								ledger_list
									.try_push(next_queue_id)
									.map_err(|_| Error::<T>::TooManyRedeems)?;

								*total_locked = total_locked
									.checked_add(&currency_amount)
									.ok_or(ArithmeticError::Overflow)?;
							};
							Ok(())
						})?;
					} else {
						let mut ledger_list_origin = BoundedVec::<QueueId, T::MaximumQueue>::default();
						ledger_list_origin
							.try_push(next_queue_id)
							.map_err(|_| Error::<T>::TooManyRedeems)?;
						UserUnlockRequest::<T>::insert(&who, &currency_id, (currency_amount, ledger_list_origin));
					}

					if let Some((_, _, _token_id)) = NetworkRedeemQueue::<T>::get(&new_staking_round, &currency_id) {
						NetworkRedeemQueue::<T>::mutate(
							&new_staking_round,
							&currency_id,
							|value| -> Result<(), Error<T>> {
								if let Some((total_locked, ledger_list, _token_id)) = value {
									ledger_list
										.try_push(next_queue_id)
										.map_err(|_| Error::<T>::TooManyRedeems)?;
									*total_locked = total_locked
										.checked_add(&currency_amount)
										.ok_or(ArithmeticError::Overflow)?;
								};
								Ok(())
							},
						)?;
					} else {
						let mut ledger_list_origin = BoundedVec::<QueueId, T::MaximumQueue>::default();
						ledger_list_origin
							.try_push(next_queue_id)
							.map_err(|_| Error::<T>::TooManyRedeems)?;

						NetworkRedeemQueue::<T>::insert(
							&new_staking_round,
							&currency_id,
							(currency_amount, ledger_list_origin, currency_id),
						);
					}
				}
				None => return Err(Error::<T>::NoCurrentStakingRound.into()),
			}

			// Emit deposit event
			Self::deposit_event(Event::Redeemed(who, pool_id, vamount));
			Ok(().into())
		}
	}
}

impl<T: Config> Pallet<T> {
	#[transactional]
	pub fn calculate_next_staking_round(a: StakingRound, b: StakingRound) -> Result<StakingRound, DispatchError> {
		let result = match a {
			StakingRound::Era(era_a) => match b {
				StakingRound::Era(era_b) => {
					StakingRound::Era(era_a.checked_add(era_b).ok_or(ArithmeticError::Overflow)?)
				}
				_ => return Err(Error::<T>::Unexpected.into()),
			},
			StakingRound::Round(round_a) => match b {
				StakingRound::Round(round_b) => {
					StakingRound::Round(round_a.checked_add(round_b).ok_or(ArithmeticError::Overflow)?)
				}
				_ => return Err(Error::<T>::Unexpected.into()),
			},
			StakingRound::Epoch(epoch_a) => match b {
				StakingRound::Epoch(epoch_b) => {
					StakingRound::Epoch(epoch_a.checked_add(epoch_b).ok_or(ArithmeticError::Overflow)?)
				}
				_ => return Err(Error::<T>::Unexpected.into()),
			},
			StakingRound::Hour(hour_a) => match b {
				StakingRound::Hour(hour_b) => {
					StakingRound::Hour(hour_a.checked_add(hour_b).ok_or(ArithmeticError::Overflow)?)
				}
				_ => return Err(Error::<T>::Unexpected.into()),
			},
		};

		Ok(result)
	}

	#[transactional]
	pub fn collect_deposit_fee(
		who: T::AccountId,
		currency_id: BalanceOf<T>,
		amount: BalanceOf<T>,
	) -> Result<BalanceOf<T>, DispatchError> {
		let (deposit_rate, _redeem_rate) = Fees::<T>::get();

		let deposit_fee = deposit_rate * amount;
		let amount_exclude_fee = amount.checked_sub(&deposit_fee).ok_or(ArithmeticError::Overflow)?;
		T::MultiCurrency::transfer(currency_id, who, &T::NetworkFee::get(), deposit_fee)?;

		return amount_exclude_fee;
	}

	#[transactional]
	pub fn collect_redeem_fee(
		who: T::AccountId,
		currency_id: BalanceOf<T>,
		amount: BalanceOf<T>,
	) -> Result<BalanceOf<T>, DispatchError> {
		let (_mint_rate, redeem_rate) = Fees::<T>::get();
		let redeem_fee = redeem_rate * amount;
		let amount_exclude_fee = amount.checked_sub(&deposit_fee).ok_or(ArithmeticError::Overflow)?;
		T::MultiCurrency::transfer(currency_id, who, &T::NetworkFee::get(), redeem_fee)?;

		return amount_exclude_fee;
	}
}
