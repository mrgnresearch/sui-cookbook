use anyhow::{anyhow, bail, ensure};
use common::fetch_sorted_gas_coins;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use sui_sdk::{rpc_types::SuiTransactionBlockEffectsAPI, SuiClientBuilder};
use sui_types::{
    balance::Balance,
    base_types::SuiAddress,
    coin::{self, Coin},
    id::{ID, UID},
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    transaction::{Argument, ObjectArg, TransactionData, TransactionKind},
    Identifier, TypeTag, SUI_FRAMEWORK_PACKAGE_ID,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ---------------------------------------------------------------------------------------------
    // Setup

    let mut pt_builder = ProgrammableTransactionBuilder::new();

    // Pick random address owning at least 2 non-empty SUI coin objects
    let sender =
        SuiAddress::from_str("0x43b8f743162704af85214b0d0159fbef11aae0e996a8e9eac7fafda7fc5bd5f2")?;

    let rpc_client = SuiClientBuilder::default()
        .build("https://fullnode.mainnet.sui.io:443")
        .await?;

    let gas_price = rpc_client.read_api().get_reference_gas_price().await?;
    let gas_coins = fetch_sorted_gas_coins(&rpc_client, &sender).await?;

    ensure!(gas_coins.len() > 1, "Need at least 2 non-empty gas coins"); // 1 for gas, 1 for the coin we're manipulating

    let (richest_coin, _) = gas_coins.first().unwrap();
    let original_coin_arg = ObjectArg::ImmOrOwnedObject(richest_coin.object_ref());
    let original_coin_arg = pt_builder.obj(original_coin_arg)?;

    let number_two_arg = pt_builder.pure(2u64)?;

    let gas_payment = gas_coins[1..]
        .iter()
        .map(|(coin, _)| coin.object_ref())
        .collect::<Vec<_>>();

    // ---------------------------------------------------------------------------------------------
    // Programmable Transaction (PT) building

    // Create some re-usable vars
    let math_module = Identifier::from_str("math")?;
    let kiosk_module = Identifier::from_str("kiosk")?;
    let sui_coin_arg_type = TypeTag::from_str("0x2::sui::SUI")?;
    let value_function = Identifier::from_str("value")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/coin.move#L86-L89
    let destroy_zero_function = Identifier::from_str("destroy_zero")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/coin.move#L223-L228

    // 0: get the balance of the provided coin
    let initial_value_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        value_function.to_owned(),
        vec![sui_coin_arg_type.to_owned()],
        vec![original_coin_arg],
    );

    // 1: calculate half the balance
    let divide_function = Identifier::from_str("divide_and_round_up")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/math.move#L136-L143
    let target_balance_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        math_module.to_owned(),
        divide_function,
        vec![],
        vec![initial_value_result, number_two_arg],
    );

    // 2: split the original coin into a new one with balance equal to the target balance
    let split_function = Identifier::from_str("split")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/coin.move#L164-L170
    let new_coin_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        split_function,
        vec![sui_coin_arg_type.clone()],
        vec![original_coin_arg, target_balance_result],
    );

    // 3: get the balance of the new coin
    let new_coin_value_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        value_function.to_owned(),
        vec![sui_coin_arg_type.to_owned()],
        vec![new_coin_result],
    );

    // 4: create an empty SUI coin
    let zero_function = Identifier::from_str("zero")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/coin.move#L217-L221
    let empty_coin_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        zero_function,
        vec![sui_coin_arg_type.to_owned()],
        vec![],
    );

    // 5: destroy the empty SUI coin
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        destroy_zero_function.to_owned().to_owned(),
        vec![sui_coin_arg_type.to_owned()],
        vec![empty_coin_result],
    ); // ignore the result, this function returns nothing

    // 6: create a new kiosk
    let new_function = Identifier::from_str("new")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/kiosk/kiosk.move#L184C1-L200
    let new_kiosk_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        kiosk_module.to_owned(),
        new_function,
        vec![],
        vec![],
    );

    // Extract the internal index of this transaction so that we can use it to refer to the
    // nested results returned by this function (tuple).
    let Argument::Result(kiosk_result) = new_kiosk_result else { bail!("This outta be a Result") };

    // 7: check if kiosk contains a specific item (here the original coin, which it does not)
    let first_item = 0;
    let kiosk_arg = Argument::NestedResult(kiosk_result.to_owned(), first_item); // Point to the first item in the nested results of the kiosk creation.
    let non_existent_id = ID::new(richest_coin.object_id);
    let non_existent_id_arg = pt_builder.pure(non_existent_id)?;
    let has_item_function = Identifier::from_str("has_item")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/kiosk/kiosk.move#L414-L417
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        kiosk_module.to_owned(),
        has_item_function,
        vec![],
        vec![kiosk_arg.to_owned(), non_existent_id_arg],
    ); // ignore the result, we will not use it in this PTB

    // 8: close the kiosk and retrieve the coin for the balance it contained
    let second_item = 1;
    let kiosk_owner_cap_arg = Argument::NestedResult(kiosk_result.to_owned(), second_item); // Point to the second item in the nested results of the kiosk creation.
    let close_and_withdraw_function = Identifier::from_str("close_and_withdraw")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/kiosk/kiosk.move#L202-L218
    let remaining_kiosk_coin_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        kiosk_module.to_owned(),
        close_and_withdraw_function,
        vec![],
        vec![kiosk_arg, kiosk_owner_cap_arg],
    );

    // 9: destroy the empty kiosk withdrawal coin
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        destroy_zero_function.to_owned(),
        vec![sui_coin_arg_type.to_owned()],
        vec![remaining_kiosk_coin_result],
    ); // ignore the result, this function returns nothing

    // 10: calculate the absolute difference between the initial value (retrieved in transaction 0)
    // and the new coin value (retrieved in transaction 3)
    let diff_function = Identifier::from_str("diff")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/math.move#L25-L32
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        math_module.to_owned(),
        diff_function,
        vec![],
        vec![new_coin_value_result, initial_value_result],
    ); // ignore the result, we will not use it in this PTB

    // 11: merge new coin into original coin
    let join_function = Identifier::from_str("join")?; // https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/packages/sui-framework/sources/coin.move#L148-L154
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        join_function,
        vec![sui_coin_arg_type.clone()],
        vec![original_coin_arg.to_owned(), new_coin_result],
    ); // ignore the result, this function returns nothing

    // 12: transfer back the original coin to the sender to avoid tx failure due to non-droppable object still existing
    pt_builder.transfer_arg(sender, original_coin_arg);

    let pt = pt_builder.finish();

    // ---------------------------------------------------------------------------------------------
    // Execution and inspection of results

    // println!("{:#?}", pt.clone()); // For the curious

    let tx_data = TransactionKind::ProgrammableTransaction(pt.to_owned());

    let response = rpc_client
        .read_api()
        .dev_inspect_transaction_block(sender, tx_data, None, None)
        .await?;

    if let Some(e) = response.error {
        println!("Transaction failed: {}", e);
        return Ok(());
    }

    let execution_results = response.results.ok_or(anyhow!("There should be results"))?;

    ensure!(
        execution_results.len() == 13,
        "There should be 13 results, one for each transaction in the block, found {}",
        execution_results.len()
    );

    let (original_coin_value_bytes, _) = execution_results[0].clone().return_values[0].clone();
    let original_coin_value: u64 = bcs::from_bytes(&original_coin_value_bytes)?;
    println!("--> tx 0");
    println!("original_coin_value: {}", original_coin_value);

    let (new_coin_value_target_bytes, _) = execution_results[1].clone().return_values[0].clone();
    let new_coin_value_target: u64 = bcs::from_bytes(&new_coin_value_target_bytes)?;
    println!("--> tx 1");
    println!("new_coin_value_target: {}", new_coin_value_target);

    let (new_coin_bytes, _) = execution_results[2].clone().return_values[0].clone();
    let new_coin: Coin = bcs::from_bytes(&new_coin_bytes)?;
    println!("--> tx 2");
    println!("new_coin: {:?}", new_coin);
    ensure!(
        new_coin.value() == new_coin_value_target,
        "New coin value should be equal to the target value"
    );

    let (new_coin_value_bytes, _) = execution_results[3].clone().return_values[0].clone();
    let new_coin_value: u64 = bcs::from_bytes(&new_coin_value_bytes)?;
    println!("--> tx 3");
    println!("new_coin_value: {}", new_coin_value);

    let (empty_coin_bytes, _) = execution_results[4].clone().return_values[0].clone();
    let zero_coin: Coin = bcs::from_bytes(&empty_coin_bytes)?;
    println!("--> tx 4");
    println!("zero_coin: {:?}", zero_coin);
    ensure!(zero_coin.value() == 0, "Empty coin value should be 0");

    println!("--> tx 5");

    println!("--> tx 6");
    let (kiosk_bytes, _) = execution_results[6].clone().return_values[0].clone();
    let kiosk: Kiosk = bcs::from_bytes(&kiosk_bytes)?;
    println!("kiosk: {:?}", kiosk);
    let (kiosk_owner_cap_bytes, _) = execution_results[6].clone().return_values[1].clone();
    let kiosk_owner_cap: KioskOwnerCap = bcs::from_bytes(&kiosk_owner_cap_bytes)?;
    println!("kiosk_owner_cap: {:?}", kiosk_owner_cap);

    println!("--> tx 7");
    let (kiosk_has_id_bytes, _) = execution_results[7].clone().return_values[0].clone();
    let kiosk_has_id: bool = bcs::from_bytes(&kiosk_has_id_bytes)?;
    println!("kiosk_has_id: {:?}", kiosk_has_id);

    println!("--> tx 8");

    println!("--> tx 9");

    println!("--> tx 10");
    let (diff_bytes, _) = execution_results[10].clone().return_values[0].clone();
    let diff: u64 = bcs::from_bytes(&diff_bytes)?;
    println!("diff: {}", diff);
    ensure!(
        diff == original_coin_value.abs_diff(new_coin_value),
        "Absolute difference should match"
    );

    println!("--> tx 11");

    println!("--> tx 12");

    // ---------------------------------------------------------------------------------------------
    // Verify dry run succeeds

    let tx_data = TransactionData::new_programmable(
        sender,
        gas_payment,
        pt,
        100_000_000,
        gas_price.to_owned(),
    );

    let response = rpc_client
        .read_api()
        .dry_run_transaction_block(tx_data)
        .await?;

    println!("Dry run status: {:#?}", response.effects.status());

    Ok(())
}

// Original move structs

// struct Kiosk has key, store {
//     id: UID,
//     /// Balance of the Kiosk - all profits from sales go here.
//     profits: Balance<SUI>,
//     /// Always point to `sender` of the transaction.
//     /// Can be changed by calling `set_owner` with Cap.
//     owner: address,
//     /// Number of items stored in a Kiosk. Used to allow unpacking
//     /// an empty Kiosk if it was wrapped or has a single owner.
//     item_count: u32,
//     /// Whether to open the UID to public. Set to `true` by default
//     /// but the owner can switch the state if necessary.
//     allow_extensions: bool
// }
//
// struct KioskOwnerCap has key, store {
//     id: UID,
//     for: ID
// }

// Mirrored structs for deserialization

#[derive(Debug, Serialize, Deserialize)]
struct Kiosk {
    id: UID,
    profits: Balance,
    owner: SuiAddress,
    item_count: u32,
    allow_extensions: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct KioskOwnerCap {
    id: UID,
    for_: ID,
}
