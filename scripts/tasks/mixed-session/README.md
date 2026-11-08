# orderbook

Invoices for the orders in `data/orders.csv`: one line per order, a discount
per order, VAT by the region it ships to.

```sh
python3 -m unittest discover -s tests     # the tests
python3 -m orderbook.nightly              # the nightly run, which writes logs/nightly.log
python3 -m orderbook.nightly --dry-run    # the same, to stdout, with no log and no ledger check
```

- `orderbook/invoice.py` — line amounts, discounts, VAT, rounding, credit notes
- `orderbook/rates.py` — VAT by region
- `orderbook/nightly.py` — reads the orders, invoices each one, logs the run,
  checks the totals against `data/ledger.csv` when there is one

Amounts are integer cents throughout. Rounding is half-up, once, at the end of
an invoice; nothing in between is rounded.
