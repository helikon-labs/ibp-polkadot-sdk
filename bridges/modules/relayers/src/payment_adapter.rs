// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Code that allows relayers pallet to be used as a delivery+dispatch payment mechanism
//! for the messages pallet.

use crate::{Config, RelayerRewards};

use bp_messages::source_chain::{MessageDeliveryAndDispatchPayment, RelayersRewards};
use frame_support::sp_runtime::SaturatedConversion;
use sp_arithmetic::traits::{Saturating, UniqueSaturatedFrom, Zero};
use sp_std::{collections::vec_deque::VecDeque, marker::PhantomData, ops::RangeInclusive};

/// Adapter that allows relayers pallet to be used as a delivery+dispatch payment mechanism
/// for the messages pallet.
pub struct MessageDeliveryAndDispatchPaymentAdapter<T, MessagesInstance>(
	PhantomData<(T, MessagesInstance)>,
);

impl<T, MessagesInstance> MessageDeliveryAndDispatchPayment<T::RuntimeOrigin, T::AccountId>
	for MessageDeliveryAndDispatchPaymentAdapter<T, MessagesInstance>
where
	T: Config + pallet_bridge_messages::Config<MessagesInstance>,
	MessagesInstance: 'static,
{
	type Error = &'static str;

	fn pay_relayers_rewards(
		_lane_id: bp_messages::LaneId,
		messages_relayers: VecDeque<bp_messages::UnrewardedRelayer<T::AccountId>>,
		confirmation_relayer: &T::AccountId,
		received_range: &RangeInclusive<bp_messages::MessageNonce>,
	) {
		let relayers_rewards = pallet_bridge_messages::calc_relayers_rewards::<T, MessagesInstance>(
			messages_relayers,
			received_range,
		);

		register_relayers_rewards::<T>(
			confirmation_relayer,
			relayers_rewards,
			// TODO (https://github.com/paritytech/parity-bridges-common/issues/1318): this shall be fixed
			// in some way. ATM the future of the `register_relayers_rewards` is not yet known
			100_000_u32.into(),
			10_000_u32.into(),
		);
	}
}

// Update rewards to given relayers, optionally rewarding confirmation relayer.
fn register_relayers_rewards<T: Config>(
	confirmation_relayer: &T::AccountId,
	relayers_rewards: RelayersRewards<T::AccountId>,
	delivery_fee: T::Reward,
	confirmation_fee: T::Reward,
) {
	// reward every relayer except `confirmation_relayer`
	let mut confirmation_relayer_reward = T::Reward::zero();
	for (relayer, messages) in relayers_rewards {
		// sane runtime configurations guarantee that the number of messages will be below
		// `u32::MAX`
		let mut relayer_reward =
			T::Reward::unique_saturated_from(messages).saturating_mul(delivery_fee);

		if relayer != *confirmation_relayer {
			// If delivery confirmation is submitted by other relayer, let's deduct confirmation fee
			// from relayer reward.
			//
			// If confirmation fee has been increased (or if it was the only component of message
			// fee), then messages relayer may receive zero reward.
			let mut confirmation_reward =
				T::Reward::saturated_from(messages).saturating_mul(confirmation_fee);
			confirmation_reward = sp_std::cmp::min(confirmation_reward, relayer_reward);
			relayer_reward = relayer_reward.saturating_sub(confirmation_reward);
			confirmation_relayer_reward =
				confirmation_relayer_reward.saturating_add(confirmation_reward);
			register_relayer_reward::<T>(&relayer, relayer_reward);
		} else {
			// If delivery confirmation is submitted by this relayer, let's add confirmation fee
			// from other relayers to this relayer reward.
			confirmation_relayer_reward =
				confirmation_relayer_reward.saturating_add(relayer_reward);
		}
	}

	// finally - pay reward to confirmation relayer
	register_relayer_reward::<T>(confirmation_relayer, confirmation_relayer_reward);
}

/// Remember that the reward shall be paid to the relayer.
fn register_relayer_reward<T: Config>(relayer: &T::AccountId, reward: T::Reward) {
	if reward.is_zero() {
		return
	}

	RelayerRewards::<T>::mutate(relayer, |old_reward: &mut Option<T::Reward>| {
		let new_reward = old_reward.unwrap_or_else(Zero::zero).saturating_add(reward);
		*old_reward = Some(new_reward);

		log::trace!(
			target: crate::LOG_TARGET,
			"Relayer {:?} can now claim reward: {:?}",
			relayer,
			new_reward,
		);
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::*;

	const RELAYER_1: AccountId = 1;
	const RELAYER_2: AccountId = 2;
	const RELAYER_3: AccountId = 3;

	fn relayers_rewards() -> RelayersRewards<AccountId> {
		vec![(RELAYER_1, 2), (RELAYER_2, 3)].into_iter().collect()
	}

	#[test]
	fn confirmation_relayer_is_rewarded_if_it_has_also_delivered_messages() {
		run_test(|| {
			register_relayers_rewards::<TestRuntime>(&RELAYER_2, relayers_rewards(), 50, 10);

			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_1), Some(80));
			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_2), Some(170));
		});
	}

	#[test]
	fn confirmation_relayer_is_rewarded_if_it_has_not_delivered_any_delivered_messages() {
		run_test(|| {
			register_relayers_rewards::<TestRuntime>(&RELAYER_3, relayers_rewards(), 50, 10);

			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_1), Some(80));
			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_2), Some(120));
			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_3), Some(50));
		});
	}

	#[test]
	fn only_confirmation_relayer_is_rewarded_if_confirmation_fee_has_significantly_increased() {
		run_test(|| {
			register_relayers_rewards::<TestRuntime>(&RELAYER_3, relayers_rewards(), 50, 1000);

			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_1), None);
			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_2), None);
			assert_eq!(RelayerRewards::<TestRuntime>::get(RELAYER_3), Some(250));
		});
	}
}
