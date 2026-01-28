║ Enhanced Multi-Market Data Logger (Backward Compatible)  ║
║ Original 23 cols + 22 new cols for latency analysis      ║
╚═══════════════════════════════════════════════════════════╝

Duration: 28800s (8.0h) | Output: custom_data.csv | Interval: 0s

⏳ Fetching current market...
2026-01-27T23:35:36.067389Z  INFO multi_market_logger: Fetching current market: btc-updown-15m-1769556600
✅ Market: btc-updown-15m-1769556600 | YES: 7716129801... | NO: 1002619007...
2026-01-27T23:35:36.192385Z  INFO combine_fetcher::api::binance: Connecting to Binance WebSocket (bookTicker): wss://stream.binance.com:9443/ws/btcusdt@bookTicker
2026-01-27T23:35:36.192395Z  INFO combine_fetcher::api::binance: Connecting to Binance WebSocket (aggTrade): wss://stream.binance.com:9443/ws/btcusdt@aggTrade
✅ Binance connected
2026-01-27T23:35:36.875240Z  INFO combine_fetcher::api::binance: Connected to Binance aggTrade WebSocket
2026-01-27T23:35:36.923831Z  INFO combine_fetcher::api::binance: Connected to Binance bookTicker WebSocket
⏳ Bootstrapping volatility with historical data...
2026-01-27T23:35:38.194513Z  INFO combine_fetcher::api::binance: 📊 Bootstrapping volatility for BTCUSDT with 70 1m klines...
2026-01-27T23:35:38.358637Z  INFO combine_fetcher::api::binance: ✅ Bootstrapped BTCUSDT with 70 klines | 1min buffer: 70 | unified buffer: 3
✅ Bootstrapped with 70 1-min candles
⏳ Fetching strike price for btc-updown-15m-1769556600...
2026-01-27T23:35:55.607035Z  WARN multi_market_logger: ⚠️ Strike fetch failed: Strike fetch failed after 20 retries - Using fallback value 0.0
⚠️  Fallback: strike_price set to 0.0
2026-01-27T23:35:55.607101Z  INFO combine_fetcher::api::polymarket: Subscribing to 2 Polymarket tokens
2026-01-27T23:35:55.607113Z  INFO combine_fetcher::api::polymarket: Polymarket orderbook stream started for 2 tokens with REST fallback
✅ Polymarket connected

2026-01-27T23:35:55.607125Z  INFO combine_fetcher::api::polymarket: [PRICE_STREAM] Task starting for 2 tokens
2026-01-27T23:35:55.607136Z  INFO combine_fetcher::api::polymarket: [PRICE_STREAM] Attempting initial connection...
2026-01-27T23:35:55.607349Z  INFO combine_fetcher::api::polymarket: [BOOK_STREAM] Task starting for 2 tokens
2026-01-27T23:35:55.607355Z  INFO combine_fetcher::api::polymarket: [BOOK_STREAM] Attempting initial connection...
2026-01-27T23:35:55.607361Z  INFO combine_fetcher::api::polymarket: [HEALTH_MONITOR] Task started (checks every 5s for silent disconnects)
2026-01-27T23:35:55.607486Z  INFO combine_fetcher::api::polymarket: [BOOK_STREAM] State transition: CONNECTING -> CONNECTED (initial connection)
2026-01-27T23:35:55.607520Z  WARN combine_fetcher::api::polymarket: [HEALTH_CHECK] Health: 0% (0/2 healthy) stale: [none] disconnected: [none]
2026-01-27T23:35:55.608374Z  INFO combine_fetcher::api::polymarket: [PRICE_STREAM] State transition: CONNECTING -> CONNECTED (initial connection)
2026-01-27T23:35:55.620945Z  INFO combine_fetcher::api::polymarket: [REST_FALLBACK] Task started (polls every 3s when WebSocket is disconnected)
2026-01-27T23:35:58.608358Z  INFO multi_market_logger: Starting enhanced data collection...


Why do you start the log when strike price fail the fetch ？ you should keep fetching it
