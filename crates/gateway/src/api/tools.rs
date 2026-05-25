use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::tools::ToolContext;

use super::{AppError, AppState};

#[derive(Deserialize)]
pub struct CandlesticksQuery {
    ticker: String,
    market: String,
    interval: Option<String>,
    limit: Option<usize>,
    adjusted: Option<bool>,
}

#[derive(Deserialize)]
pub struct FinancialsQuery {
    ticker: String,
    market: String,
    report_type: Option<String>,
    periods: Option<usize>,
}

#[derive(Deserialize)]
pub struct QuoteQuery {
    tickers: String,
    market: Option<String>,
}

#[derive(Serialize)]
pub struct ToolPreview {
    output_preview: String,
}

pub async fn candlesticks_handler(
    State(state): State<AppState>,
    Query(params): Query<CandlesticksQuery>,
) -> Result<Json<ToolPreview>, AppError> {
    let args = serde_json::json!({
        "ticker": params.ticker,
        "market": params.market,
        "interval": params.interval.unwrap_or_else(|| "1d".to_string()),
        "limit": params.limit.unwrap_or(180),
        "adjusted": params.adjusted.unwrap_or(true),
    });
    let ctx = ToolContext {
        pool: state.pool.clone(),
        event_bus: state.event_bus.clone(),
        user_id: state.user_id.clone(),
        session_id: "__card__".to_string(),
        task_id: None,
    };
    let output = state
        .tools
        .dispatch(
            "get_candlesticks",
            &args.to_string(),
            CancellationToken::new(),
            &ctx,
        )
        .await?;
    Ok(Json(ToolPreview {
        output_preview: output,
    }))
}

pub async fn financials_handler(
    State(state): State<AppState>,
    Query(params): Query<FinancialsQuery>,
) -> Result<Json<ToolPreview>, AppError> {
    let args = serde_json::json!({
        "ticker": params.ticker,
        "market": params.market,
        "report_type": params.report_type.unwrap_or_else(|| "all".to_string()),
        "periods": params.periods.unwrap_or(12),
    });
    let ctx = ToolContext {
        pool: state.pool.clone(),
        event_bus: state.event_bus.clone(),
        user_id: state.user_id.clone(),
        session_id: "__card__".to_string(),
        task_id: None,
    };
    let output = state
        .tools
        .dispatch(
            "get_financials",
            &args.to_string(),
            CancellationToken::new(),
            &ctx,
        )
        .await?;
    Ok(Json(ToolPreview {
        output_preview: output,
    }))
}

pub async fn quote_handler(
    State(state): State<AppState>,
    Query(params): Query<QuoteQuery>,
) -> Result<Json<ToolPreview>, AppError> {
    let tickers: Vec<String> = params
        .tickers
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let args = serde_json::json!({
        "tickers": tickers,
        "market": params.market.unwrap_or_else(|| "america".to_string()),
    });
    let ctx = ToolContext {
        pool: state.pool.clone(),
        event_bus: state.event_bus.clone(),
        user_id: state.user_id.clone(),
        session_id: "__card__".to_string(),
        task_id: None,
    };
    let output = state
        .tools
        .dispatch(
            "market_quote",
            &args.to_string(),
            CancellationToken::new(),
            &ctx,
        )
        .await?;
    Ok(Json(ToolPreview {
        output_preview: output,
    }))
}
