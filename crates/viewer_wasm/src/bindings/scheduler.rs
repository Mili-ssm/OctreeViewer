use std::collections::HashMap;

use js_sys::{Array, Object, Reflect};
use viewer_core::{BlockId, BlockRequestScheduler, RequestTicket};
use wasm_bindgen::prelude::*;

/// WASM façade over the core block scheduler used by the Blob-backed LOD worker.
#[wasm_bindgen]
pub struct WasmBlockRequestScheduler {
    inner: BlockRequestScheduler,
    tickets: HashMap<u64, RequestTicket>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "wasm-bindgen exports receive JS handles and owned strings by value"
)]
#[wasm_bindgen]
impl WasmBlockRequestScheduler {
    #[wasm_bindgen(constructor)]
    pub fn new(max_in_flight: u32) -> Result<Self, JsValue> {
        let inner = BlockRequestScheduler::new(max_in_flight as usize)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(Self {
            inner,
            tickets: HashMap::new(),
        })
    }

    /// Reconciles the existing LOD priority order; the first ID has highest priority.
    #[wasm_bindgen(js_name = updateBlocks)]
    pub fn update_blocks(&mut self, block_ids: Array) -> Result<Array, JsValue> {
        let requests = block_ids
            .iter()
            .enumerate()
            .map(|(priority, value)| {
                let block_id = parse_id(&value, "block ID")?;
                Ok((BlockId::new(block_id), priority as f64))
            })
            .collect::<Result<Vec<_>, JsValue>>()?;
        let update = self
            .inner
            .update_blocks(requests)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(ticket_ids(update.cancelled()))
    }

    #[wasm_bindgen(js_name = pendingBlocks)]
    pub fn pending_blocks(&self) -> Array {
        self.inner
            .pending_blocks()
            .iter()
            .map(|id| JsValue::from_str(&id.get().to_string()))
            .collect()
    }

    /// Admits the best-priority pending request among the IDs whose byte reservation succeeded.
    #[wasm_bindgen(js_name = takeNextAdmitted)]
    pub fn take_next_admitted(&mut self, admitted_blocks: Array) -> Result<JsValue, JsValue> {
        let block_ids = admitted_blocks
            .iter()
            .map(|value| parse_id(&value, "admitted block ID").map(BlockId::new))
            .collect::<Result<Vec<_>, _>>()?;
        let Some(request) = self
            .inner
            .take_next_admitted(&block_ids)
            .map_err(|error| JsValue::from_str(&error.to_string()))?
        else {
            return Ok(JsValue::NULL);
        };
        let ticket = request.ticket();
        self.tickets.insert(ticket.id(), ticket);
        let value = Object::new();
        Reflect::set(
            &value,
            &JsValue::from_str("ticket"),
            &JsValue::from_str(&ticket.id().to_string()),
        )?;
        Reflect::set(
            &value,
            &JsValue::from_str("blockId"),
            &JsValue::from_str(&ticket.block_id().get().to_string()),
        )?;
        Ok(value.into())
    }

    #[wasm_bindgen(js_name = complete)]
    pub fn complete(
        &mut self,
        ticket_id: String,
        actual_block_id: String,
        succeeded: bool,
    ) -> Result<String, JsValue> {
        let ticket_id = ticket_id
            .parse::<u64>()
            .map_err(|_| JsValue::from_str("ticket ID must be a u64 string"))?;
        let actual_block_id = parse_id(&JsValue::from_str(&actual_block_id), "block ID")?;
        let ticket = self
            .tickets
            .remove(&ticket_id)
            .ok_or_else(|| JsValue::from_str("ticket is unknown or already completed"))?;
        let outcome = self
            .inner
            .complete(
                ticket,
                if succeeded {
                    Ok(BlockId::new(actual_block_id))
                } else {
                    Err(())
                },
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(match outcome {
            viewer_core::RequestCompletion::Delivered => "delivered",
            viewer_core::RequestCompletion::Discarded => "discarded",
            viewer_core::RequestCompletion::Failed => "failed",
            viewer_core::RequestCompletion::UnexpectedBlock { .. } => "unexpectedBlock",
        }
        .to_string())
    }

    #[wasm_bindgen(js_name = isCurrent)]
    pub fn is_current(&self, ticket_id: String) -> Result<bool, JsValue> {
        let id = ticket_id
            .parse::<u64>()
            .map_err(|_| JsValue::from_str("ticket ID must be a u64 string"))?;
        let ticket = self
            .tickets
            .get(&id)
            .copied()
            .ok_or_else(|| JsValue::from_str("ticket is unknown or already completed"))?;
        Ok(self.inner.ticket_is_current(ticket))
    }

    #[wasm_bindgen(js_name = retry)]
    pub fn retry(&mut self, block_id: String) -> Result<bool, JsValue> {
        let id = parse_id(&JsValue::from_str(&block_id), "block ID")?;
        Ok(self.inner.retry(BlockId::new(id)))
    }

    #[wasm_bindgen(js_name = fail)]
    pub fn fail(&mut self, block_id: String) -> Result<bool, JsValue> {
        let id = parse_id(&JsValue::from_str(&block_id), "block ID")?;
        Ok(self.inner.fail(BlockId::new(id)))
    }

    #[wasm_bindgen(js_name = cancelAll)]
    pub fn cancel_all(&mut self) -> Array {
        ticket_ids(&self.inner.cancel_all())
    }

    #[wasm_bindgen(js_name = inFlightCount)]
    pub fn in_flight_count(&self) -> usize {
        self.inner.in_flight_count()
    }
}

fn parse_id(value: &JsValue, label: &str) -> Result<u64, JsValue> {
    value
        .as_string()
        .ok_or_else(|| JsValue::from_str(&format!("{label} must be a string")))?
        .parse::<u64>()
        .map_err(|_| JsValue::from_str(&format!("{label} must be a u64 string")))
}

fn ticket_ids(tickets: &[RequestTicket]) -> Array {
    tickets
        .iter()
        .map(|ticket| JsValue::from_str(&ticket.id().to_string()))
        .collect()
}
