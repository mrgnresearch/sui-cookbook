use std::str::FromStr;
use anyhow::{bail, ensure};
use sui_types::{
    base_types::SuiAddress,
    Identifier,
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    TypeTag,
    SUI_FRAMEWORK_PACKAGE_ID,
    coin,
    transaction::{ObjectArg, TransactionData},
    transaction::Argument,
};
use sui_sdk::{SuiClientBuilder};
use sui_sdk::rpc_types::SuiExecutionStatus::Success;
use sui_types::transaction::Command;
use common::fetch_sorted_gas_coins;
use sui_sdk::rpc_types::SuiTransactionBlockEffectsAPI;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sender = SuiAddress::from_str("0xe719405821d7bd32ded86a2aed34f06f3dacd09c91241ec3f34b219ebeddc6f0")?;

    let rpc_client = SuiClientBuilder::default()
        .build("https://fullnode.mainnet.sui.io:443")
        .await?;

    let gas_budget = 100_000_000; // 0.1 Sui
    let gas_price = rpc_client.read_api().get_reference_gas_price().await?;

    let gas_coins = fetch_sorted_gas_coins(&rpc_client, &sender).await?;
    ensure!(gas_coins.len() > 0, "Need at least 1 non-empty gas coin");

    let (primary_coin, primary_coin_balance) = gas_coins.first().unwrap();
    ensure!(primary_coin_balance > &gas_budget, "Need the primary coin to have at least as much balance as the gas budget");

    // ---------------------------------------------------------------------------------------------
    // Build naive PTB

    let mut pt_builder = ProgrammableTransactionBuilder::new();
    let primary_coin_arg = pt_builder.obj(ObjectArg::ImmOrOwnedObject(primary_coin.object_ref()))?;

    // provide primary coin as unique gas payment object
    let gas_payment = vec![primary_coin.object_ref()];

    // add logic making use of the primary coin in PTB commands
    build_pt_logic(&mut pt_builder, &sender, primary_coin_arg)?;

    // finalize PTB and specify gas parameters
    let bad_pt = pt_builder.finish();
    println!("Bad PTB inputs: {:?}", bad_pt.inputs);
    let bad_tx_data = TransactionData::new_programmable(
        sender,
        gas_payment,
        bad_pt,
        gas_budget.to_owned(),
        gas_price.to_owned(),
    );

    // ---------------------------------------------------------------------------------------------
    // Simulate naive PTB

    let result = rpc_client
        .read_api()
        .dry_run_transaction_block(bad_tx_data)
        .await;

    match result {
        Ok(_) => bail!("Huh"),
        Err(e) => println!("Bad PTB failed as expected: {}", e),
    }

    // ---------------------------------------------------------------------------------------------
    // Build correct PTB

    let mut pt_builder = ProgrammableTransactionBuilder::new();

    // provide primary coin as unique gas payment object
    let gas_payment = vec![primary_coin.object_ref()];

    // split the primary into a new usable coin for the PTB logic
    let usable_balance = pt_builder.pure(primary_coin_balance - gas_budget)?;
// N.B.: the `Argument::GasCoin` points to the coin used for gas payment, which coincides with
// `primary_coin_arg` here. However it would be incorrect to specify `primary_coin_arg` explicitly,
// and would result in the same duplicate issue
    let Argument::Result(split_coin_result) = pt_builder.command(Command::SplitCoins(Argument::GasCoin, vec![usable_balance])) else { bail!("This outta be a Result") };
    let usable_coin = Argument::NestedResult(split_coin_result, 0);

    // do stuff
    build_pt_logic(&mut pt_builder, &sender, usable_coin)?;

    // finalize PTB and specify gas parameters
    let good_pt = pt_builder.finish();
    println!("-------------------------------------------");
    println!("Good PTB inputs: {:?}", good_pt.inputs);
    let good_tx_data = TransactionData::new_programmable(
        sender,
        gas_payment,
        good_pt,
        gas_budget.to_owned(),
        gas_price.to_owned(),
    );

    // ---------------------------------------------------------------------------------------------
    // Simulate correct PTB

    let result = rpc_client
        .read_api()
        .dry_run_transaction_block(good_tx_data)
        .await;

    match result {
        Ok(response) if response.effects.status() == &Success => println!("Good PTB succeeded as expected"),
        Ok(response) => bail!("Huh: {:?}", response),
        Err(e) => bail!("Huh {}", e),
    }

    Ok(())
}

fn build_pt_logic(pt_builder: &mut ProgrammableTransactionBuilder, sender: &SuiAddress, coin: Argument) -> anyhow::Result<()> {
    // get the balance of the provided coin
    let coin_value_result = pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        coin::COIN_MODULE_NAME.to_owned(),
        Identifier::from_str("value")?.to_owned(),
        vec![TypeTag::from_str("0x2::sui::SUI")?],
        vec![coin],
    );

    // calculate a ninth of the balance
    let denominator_arg = pt_builder.pure(9u64)?;
    let divide_function = Identifier::from_str("divide_and_round_up")?;
    pt_builder.programmable_move_call(
        SUI_FRAMEWORK_PACKAGE_ID,
        Identifier::from_str("math")?,
        divide_function,
        vec![],
        vec![coin_value_result, denominator_arg],
    ); // do nothing with result cause we don't care

    pt_builder.transfer_args(sender.clone(), vec![coin]);

    Ok(())
}