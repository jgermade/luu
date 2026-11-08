"""The nightly run: invoice every order in data/orders.csv, log each one.

    python3 -m orderbook.nightly                 # invoice, log, check the ledger
    python3 -m orderbook.nightly --dry-run       # invoice and log, write nothing else
    python3 -m orderbook.nightly --ledger PATH   # check against another ledger file

It stops at the first order it cannot invoice and exits non-zero, so the log's
last lines are always the reason a run did not finish. That is deliberate and
has been argued about more than once: invoice numbers are gapless (see
`InvoiceNumbers`), so skipping an order and carrying on would number every
later invoice one lower than the ledger expects, and the morning's check would
report every region as wrong instead of the one order that was. Stopping keeps
the damage to one line in the log. Skipping would need the numbers taken after
the whole batch is known to invoice, which is a change to how the run is built
rather than to this loop.
"""

import argparse
import csv
import sys
import traceback
from datetime import datetime, timedelta
from pathlib import Path

from orderbook.invoice import (
    InvoiceNumbers,
    Line,
    Order,
    check_against_ledger,
    format_cents,
    make_invoice,
    summarise,
)

ROOT = Path(__file__).resolve().parent.parent
ORDERS = ROOT / "data" / "orders.csv"
LEDGER = ROOT / "data" / "ledger.csv"
LOG = ROOT / "logs" / "nightly.log"


def read_orders(path=ORDERS):
    """Every order in `path`, one line each, in file order."""
    with open(path, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            yield Order(
                order_id=row["order_id"],
                customer=row["customer"],
                region=row["region"],
                discount_pct=int(row["discount_pct"]),
                lines=[Line(row["sku"], int(row["qty"]), int(row["unit_cents"]))],
            )


def read_ledger(path=LEDGER):
    """What the ledger booked per region, in cents. Absent file, empty ledger."""
    try:
        with open(path, newline="", encoding="utf-8") as f:
            return {row["region"]: int(row["total_cents"]) for row in csv.DictReader(f)}
    except FileNotFoundError:
        return {}


def run(log, clock=datetime.now, orders=None, ledger=None, numbers=None):
    """Invoice every order, writing one line per invoice to `log`.

    `clock` is called once per line, so a test can pass one that ticks.
    Returns the process's exit code: 0 for a run that invoiced everything and
    matched the ledger, 1 for one that stopped, 2 for one the ledger disputes.
    """

    def say(level, message):
        log.write(f"{clock():%Y-%m-%d %H:%M:%S} {level:<5} nightly: {message}\n")

    orders = list(read_orders() if orders is None else orders)
    numbers = numbers or InvoiceNumbers(clock().year)
    say("INFO", f"start: {len(orders)} orders in data/orders.csv")
    invoices = []
    for order in orders:
        try:
            invoice = make_invoice(order)
        except Exception:
            say("ERROR", f"order {order.order_id} ({order.customer}): cannot invoice")
            for line in traceback.format_exc().rstrip().splitlines():
                say("ERROR", line)
            say("INFO", f"stopped after {len(invoices)} of {len(orders)} orders")
            return 1
        invoices.append(invoice)
        numbers.take(order.placed_on)
        say(
            "INFO",
            f"invoiced {invoice.order_id} {invoice.region} "
            f"net {format_cents(invoice.net_cents)} "
            f"{invoice.tax_label} {format_cents(invoice.tax_cents)} "
            f"total {format_cents(invoice.total_cents)}",
        )
    say("INFO", f"done: {len(invoices)} of {len(orders)} orders")

    if ledger is None:
        return 0
    problems = check_against_ledger(summarise(invoices), ledger)
    for problem in problems:
        say("WARN", f"ledger: {problem}")
    if problems:
        say("WARN", f"ledger: {len(problems)} region(s) disagree")
        return 2
    say("INFO", "ledger: every region agrees")
    return 0


def ticking(start, step=timedelta(milliseconds=731)):
    """A clock for tests: `start`, then `step` later on every call."""
    now = [start - step]

    def clock():
        now[0] += step
        return now[0]

    return clock


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python3 -m orderbook.nightly")
    parser.add_argument("--orders", type=Path, default=ORDERS)
    parser.add_argument("--ledger", type=Path, default=LEDGER)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="invoice and log to stdout; skip the ledger and the log file",
    )
    args = parser.parse_args(argv)
    orders = list(read_orders(args.orders))
    if args.dry_run:
        return run(sys.stdout, orders=orders)
    LOG.parent.mkdir(exist_ok=True)
    with open(LOG, "a", encoding="utf-8") as log:
        return run(log, orders=orders, ledger=read_ledger(args.ledger))


if __name__ == "__main__":
    sys.exit(main())
