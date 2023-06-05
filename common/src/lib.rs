use sui_sdk::{
    rpc_types::{SuiObjectData, SuiObjectDataFilter, SuiObjectDataOptions, SuiObjectResponseQuery},
    SuiClient
};
use sui_types::{
    gas_coin::GasCoin,
    base_types::SuiAddress
};

pub async fn fetch_sorted_gas_coins(rpc_client: &SuiClient, sender: &SuiAddress) -> anyhow::Result<Vec<(SuiObjectData, u64)>> {
    let mut gas_objects: Vec<(SuiObjectData, u64)> = vec![];
    let mut cursor = None;
    loop {
        let response = rpc_client
            .read_api()
            .get_owned_objects(
                sender.clone(),
                Some(SuiObjectResponseQuery {
                    filter: Some(SuiObjectDataFilter::MatchAll(vec![
                        SuiObjectDataFilter::StructType(GasCoin::type_()),
                    ])),
                    options: Some(SuiObjectDataOptions::full_content()),
                }),
                cursor,
                None,
            )
            .await?;

        let new_gas_objects: Vec<_> = response.data.into_iter().filter_map(
            |maybe_object|
                if let Some(object) = maybe_object.data {
                    let gas_coin = GasCoin::try_from(&object).unwrap();
                    let gas_balance = gas_coin.value();
                    if gas_balance > 0 { Some((object, gas_balance)) } else { None }
                } else { None }
        ).collect();

        gas_objects.extend(new_gas_objects);

        if !response.has_next_page { break; };
        cursor = response.next_cursor;
    }

    gas_objects.sort_by(|(_, a), (_, b)| b.cmp(a));

    Ok(gas_objects)
}
