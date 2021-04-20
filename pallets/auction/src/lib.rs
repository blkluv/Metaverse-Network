// This pallet use The Open Runtime Module Library (ORML) which is a community maintained collection of Substrate runtime modules.
// Thanks to all contributors of orml.
// Ref: https://github.com/open-web3-stack/open-runtime-module-library

#![cfg_attr(not(feature = "std"), no_std)]
// Disable the following two lints since they originate from an external macro (namely decl_storage)
#![allow(clippy::string_lit_as_bytes)]

use frame_support::{
    decl_error, decl_event, decl_module, decl_storage, ensure,
    traits::{Currency, ExistenceRequirement, Get, ReservableCurrency},
    IterableStorageDoubleMap, Parameter,
    debug,
};

use codec::{Decode, Encode};
use sp_runtime::{
    traits::{AtLeast32BitUnsigned, Bounded, MaybeSerializeDeserialize, Member, One, Zero},
    DispatchError, DispatchResult, RuntimeDebug,
};

use frame_system::{self as system, ensure_signed};
use pallet_nft::Module as NFTModule;
use pallet_continuum::Pallet as ContinuumModule;

use primitives::{ItemId, AuctionId};

use auction_manager;

pub use crate::auction::{Auction, AuctionHandler, Change, OnNewBidResult};
use auction_manager::{OnNewBidResult, AuctionHandler, Change};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mock;

pub struct AuctionLogicHandler;

#[cfg_attr(feature = "std", derive(PartialEq, Eq))]
#[derive(Encode, Decode, Clone, RuntimeDebug)]
pub struct AuctionItem<AccountId, BlockNumber, Balance> {
    item_id: ItemId,
    recipient: AccountId,
    initial_amount: Balance,
    /// Current amount for sale
    amount: Balance,
    /// Auction start time
    start_time: BlockNumber,
    end_time: BlockNumber,
}

/// Auction info.
#[cfg_attr(feature = "std", derive(PartialEq, Eq))]
#[derive(Encode, Decode, Clone, RuntimeDebug)]
pub struct AuctionInfo<AccountId, Balance, BlockNumber> {
    /// Current bidder and bid price.
    pub bid: Option<(AccountId, Balance)>,
    /// Define which block this auction will be started.
    pub start: BlockNumber,
    /// Define which block this auction will be ended.
    pub end: Option<BlockNumber>,
}

type ClassIdOf<T> = <T as orml_nft::Config>::ClassId;
type TokenIdOf<T> = <T as orml_nft::Config>::TokenId;
type BalanceOf<T> =
<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait Config:
frame_system::Config
+ pallet_nft::Config
+ pallet_balances::Config
{
    type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;
    type AuctionTimeToClose: Get<Self::BlockNumber>;
    /// The `AuctionHandler` that allow custom bidding logic and handles auction
    /// result
    type Handler: AuctionHandler<Self::AccountId, Self::Balance, Self::BlockNumber, AuctionId>;
    type Currency: Currency<Self::AccountId>;

    // /// Weight information for extrinsics in this module.
    // type WeightInfo: WeightInfo;
}

decl_storage! {
    trait Store for Module<T: Config> as Auction {
        /// Stores on-going and future auctions. Closed auction are removed.
        pub Auctions get(fn auctions): map hasher(twox_64_concat) AuctionId => Option<AuctionInfo<T::AccountId, T::Balance, T::BlockNumber>>;

        //Store asset with Auction
        pub AuctionItems get(fn get_auction_item): map hasher(twox_64_concat) AuctionId => Option<AuctionItem<T::AccountId, T::BlockNumber, T::Balance>>;

        /// Track the next auction ID.
        pub AuctionsIndex get(fn auctions_index): AuctionId;

        /// Index auctions by end time.
        pub AuctionEndTime get(fn auction_end_time): double_map hasher(twox_64_concat) T::BlockNumber, hasher(twox_64_concat) AuctionId => Option<()>;
    }
}
decl_event!(
    pub enum Event<T> where
        <T as frame_system::Config>::AccountId,
        <T as pallet_balances::Config>::Balance,
        // AssetId = AssetId,
    {
        /// A bid is placed. [auction_id, bidder, bidding_amount]
        Bid(AuctionId, AccountId, Balance),
        NewAuctionItem(AuctionId, AccountId ,Balance, Balance),
        AuctionFinalized(AuctionId, AccountId, Balance),
    }
);

decl_module! {
    pub struct Module<T: Config> for enum Call where origin: T::Origin {
        type Error = Error<T>;
        fn deposit_event() = default;

        /// The extended time for the auction to end after each successful bid
        const AuctionTimeToClose: T::BlockNumber = T::AuctionTimeToClose::get();

        #[weight = 10_000]
        fn bid(origin, id: AuctionId, value: T::Balance) {
            let from = ensure_signed(origin)?;

            <Auctions<T>>::try_mutate_exists(id, |auction| -> DispatchResult {
                let mut auction = auction.as_mut().ok_or(Error::<T>::AuctionNotExist)?;

                let block_number = <frame_system::Module<T>>::block_number();

                // make sure auction is started
                ensure!(block_number >= auction.start, Error::<T>::AuctionNotStarted);

                let auction_end: Option<T::BlockNumber> = auction.end;

                ensure!(block_number < auction_end.unwrap(), Error::<T>::AuctionIsExpired);

                if let Some(ref current_bid) = auction.bid {
                    ensure!(value > current_bid.1, Error::<T>::InvalidBidPrice);
                } else {
                    ensure!(!value.is_zero(), Error::<T>::InvalidBidPrice);
                }
                let bid_result = T::Handler::on_new_bid(
                    block_number,
                    id,
                    (from.clone(), value),
                    auction.bid.clone(),
                );

                ensure!(bid_result.accept_bid, Error::<T>::BidNotAccepted);

                ensure!(<pallet_balances::Module<T>>::free_balance(&from) >= value, "You don't have enough free balance for this bid");

                Self::auction_bid_handler(block_number, id, (from.clone(), value), auction.bid.clone())?;

                auction.bid = Some((from.clone(), value));
                Self::deposit_event(RawEvent::Bid(id, from, value));

                Ok(())
            })?;

        }

        #[weight = 10_000]
        fn create_new_auction(origin, item_id: ItemId, value: T::Balance) {
            let from = ensure_signed(origin)?;

            let start_time: T::BlockNumber = <system::Module<T>>::block_number();
            let end_time: T::BlockNumber = start_time + T::AuctionTimeToClose::get(); //add 7 days block for default auction

            let auction_id = Self::create_auction(item_id, end_time, from, value, start_time);
            Self::deposit_event(RawEvent::NewAuctionItem(auction_id, from, value ,value));
        }

        /// dummy `on_initialize` to return the weight used in `on_finalize`.
        // fn on_initialize(now: T::BlockNumber) -> Weight {
        // 	T::WeightInfo::on_finalize(<AuctionEndTime<T>>::iter_prefix(&now).count() as u32)
        // }

        fn on_finalize(now: T::BlockNumber) {
            for (auction_id, _) in <AuctionEndTime<T>>::drain_prefix(&now) {
                if let Some(auction) = <Auctions<T>>::get(&auction_id) {
                        if let Some(auction_item) = <AuctionItems<T>>::get(&auction_id){
                            Self::remove_auction(auction_id.clone());
                            //Transfer balance from high bidder to asset owner
                            if let Some(current_bid) = auction.bid{
                                let (high_bidder, high_bid_price): (T::AccountId, T::Balance) = current_bid;
                                <pallet_balances::Module<T>>::unreserve(&high_bidder, high_bid_price);
                                let currency_transfer = <pallet_balances::Module<T> as Currency<_>>::transfer(&high_bidder, &auction_item.recipient , high_bid_price, ExistenceRequirement::KeepAlive);
                                match currency_transfer {
                                    Err(_e) => continue,
                                    Ok(_v) => {
                                        //Transfer asset from asset owner to high bidder
                                        //Check asset type and handle internal logic

                                    match auction_item.item_id {
                                            ItemId::NFT(asset_id) => {
                                                let asset_transfer = NFTModule::<T>::do_transfer(&auction_item.recipient, &high_bidder, asset_id);
                                                   match asset_transfer {
                                                        Err(_) => continue,
                                                        Ok(_) => {
                                                            Self::deposit_event(RawEvent::AuctionFinalized(auction_id, high_bidder ,high_bid_price));
                                                        },
                                                    }
                                            }
                                            ItemId::Spot(spot_id, country_id) => {
                                                let continuum_spot = ContinuumModule::<T>::transfer_spot(&spot_id, &auction_item.recipient, &(high_bidder, country_id));
                                                match continuum_spot{
                                                     Err(_) => continue,
                                                     Ok(_) => {
                                                            Self::deposit_event(RawEvent::AuctionFinalized(auction_id, high_bidder ,high_bid_price));
                                                     },
                                                }
                                            }
                                            }
                                            _ => {} //Future implementation for Spot, Country
                                        }
                                    },
                                }
                            }
                        }
                }
            }
        }
    }
}

decl_error! {
    /// Error for auction module.
    pub enum Error for Module<T: Config> {
        AuctionNotExist,
        AssetIsNotExist,
        AuctionNotStarted,
        AuctionIsExpired,
        BidNotAccepted,
        InvalidBidPrice,
        NoAvailableAuctionId,
        NoPermissionToCreateAuction,
        AuctionTypeIsNotSupported,
    }
}

impl<T: Config> Module<T> {
    fn update_auction(
        id: AuctionId,
        info: AuctionInfo<T::AccountId, T::Balance, T::BlockNumber>,
    ) -> DispatchResult {
        let auction = <Auctions<T>>::get(id).ok_or(Error::<T>::AuctionNotExist)?;
        if let Some(old_end) = auction.end {
            <AuctionEndTime<T>>::remove(&old_end, id);
        }
        if let Some(new_end) = info.end {
            <AuctionEndTime<T>>::insert(&new_end, id, ());
        }
        <Auctions<T>>::insert(id, info);
        Ok(())
    }

    fn new_auction(
        _recipient: T::AccountId,
        _initial_amount: T::Balance,
        start: T::BlockNumber,
        end: Option<T::BlockNumber>,
    ) -> Result<AuctionId, DispatchError> {
        let auction: AuctionInfo<T::AccountId, T::Balance, T::BlockNumber> = AuctionInfo {
            bid: None,
            start,
            end,
        };

        let auction_id: AuctionId =
            AuctionsIndex::try_mutate(|n| -> Result<AuctionId, DispatchError> {
                let id = *n;
                ensure!(
                    id != AuctionId::max_value(),
                    Error::<T>::NoAvailableAuctionId
                );
                *n = n
                    .checked_add(One::one())
                    .ok_or(Error::<T>::NoAvailableAuctionId)?;
                Ok(id)
            })?;

        <Auctions<T>>::insert(auction_id, auction);

        if let Some(end_block) = end {
            <AuctionEndTime<T>>::insert(&end_block, auction_id, ());
        }

        Ok(auction_id)
    }

    fn create_auction(
        item_id: ItemId,
        end: Option<BlockNumber>,
        recipient: T::AccountId,
        initial_amount: T::Balance,
        start: T::BlockNumber,
    ) -> Result<AuctionId, DispatchError> {
        match item_id {
            ItemId::NFT(asset_id) => {
                //FIXME - Remove in prod - For debugging purpose
                debug::info!("Asset id {}", asset_id);
                //Get asset detail
                let asset = NFTModule::<T>::get_asset(asset_id).ok_or(Error::<T>::AssetIsNotExist)?;
                //Check ownership
                let class_info = orml_nft::Pallet::<T>::classes(asset.0).ok_or(Error::<T>::NoPermissionToCreateAuction)?;
                ensure!(from == class_info.owner, Error::<T>::NoPermissionToCreateAuction);
                let class_info_data = class_info.data;
                ensure!(class_info_data.token_type.is_transferrable(), Error::<T>::NoPermissionToCreateAuction);

                let start_time = <system::Module<T>>::block_number();
                let end_time: T::BlockNumber = start_time + T::AuctionTimeToClose::get(); //add 7 days block for default auction
                let auction_id = Self::new_auction(from.clone(), value, start_time, Some(end_time))?;

                let new_auction_item = AuctionItem {
                    item_id,
                    recipient: from.clone(),
                    initial_amount: value,
                    amount: value,
                    start_time,
                    end_time,
                };

                <AuctionItems<T>>::insert(
                    auction_id,
                    new_auction_item,
                );

                Self::deposit_event(RawEvent::NewAuctionItem(auction_id, from, value, value));

                Ok(auction_id)
            }
            ItemId::Spot(_spot_id, _country_id) => {
                //TODO Check if spot_id is not owned by any
                let start_time = <system::Module<T>>::block_number();
                let end_time: T::BlockNumber = start_time + T::AuctionTimeToClose::get(); //add 7 days block for default auction
                let auction_id = Self::new_auction(from.clone(), value, start_time, Some(end_time))?;

                let new_auction_item = AuctionItem {
                    item_id,
                    recipient: from.clone(),
                    initial_amount: value,
                    amount: value,
                    start_time,
                    end_time,
                };

                <AuctionItems<T>>::insert(
                    auction_id,
                    new_auction_item,
                );

                Self::deposit_event(RawEvent::NewAuctionItem(auction_id, from, value, value));

                Ok(auction_id)
            }
            _ => {}
        }
    }

    fn remove_auction(id: AuctionId) {
        if let Some(auction) = <Auctions<T>>::get(&id) {
            if let Some(end_block) = auction.end {
                <AuctionEndTime<T>>::remove(end_block, id);
                <Auctions<T>>::remove(&id)
            }
        }
    }

    /// increment `new_bidder` reference and decrement `last_bidder` reference
    /// if any
    fn swap_bidders(new_bidder: &T::AccountId, last_bidder: Option<&T::AccountId>) {
        system::Module::<T>::inc_consumers(new_bidder);

        if let Some(who) = last_bidder {
            system::Module::<T>::dec_consumers(who);
        }
    }

    fn auction_bid_handler(
        _now: T::BlockNumber,
        id: AuctionId,
        new_bid: (T::AccountId, T::Balance),
        last_bid: Option<(T::AccountId, T::Balance)>,
    ) -> DispatchResult {
        let (new_bidder, new_bid_price) = new_bid;
        ensure!(!new_bid_price.is_zero(), Error::<T>::InvalidBidPrice);

        <AuctionItems<T>>::try_mutate_exists(id, |auction_item| -> DispatchResult {
            let mut auction_item = auction_item.as_mut().ok_or("Auction is not exists")?;

            let last_bid_price = last_bid.clone().map_or(Zero::zero(), |(_, price)| price); //get last bid price
            let last_bidder = last_bid.as_ref().map(|(who, _)| who);

            if let Some(last_bidder) = last_bidder {
                //unlock reserve amount
                if !last_bid_price.is_zero() {
                    //Unreserve balance of last bidder
                    <pallet_balances::Module<T>>::unreserve(&last_bidder, last_bid_price);
                }
            }

            //Lock fund of new bidder
            //Reserve balance
            <pallet_balances::Module<T>>::reserve(&new_bidder, new_bid_price)?;
            auction_item.amount = new_bid_price.clone();

            Ok(())
        })
    }
}

impl<T: Config> AuctionHandler<T::AccountId, T::Balance, T::BlockNumber, AuctionId>
for Module<T>
{
    fn on_new_bid(
        _now: T::BlockNumber,
        _id: AuctionId,
        _new_bid: (T::AccountId, T::Balance),
        _last_bid: Option<(T::AccountId, T::Balance)>,
    ) -> OnNewBidResult<T::BlockNumber> {
        OnNewBidResult {
            accept_bid: true,
            auction_end_change: Change::NoChange,
        }
    }

    fn on_auction_ended(_id: AuctionId, _winner: Option<(T::AccountId, T::Balance)>) {}
}
