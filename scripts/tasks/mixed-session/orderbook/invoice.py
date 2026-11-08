"""Invoices: line amounts, a discount per order, VAT by region, one rounding.

Every amount is integer cents on the way in and on the way out. In between it
is a Fraction, so nothing is rounded until `round_cents` is called once, on the
invoice's total. Rounding a line, then a subtotal, then the tax, would drift by
a cent on one invoice in a few hundred, and the nightly run would disagree with
the ledger it is checked against.

The seller is in `HOME_REGION`. A business customer elsewhere in the EU who
gives a valid VAT id is invoiced without VAT (reverse charge): they account
for it themselves, and the invoice has to say so.
"""

import re
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import date, timedelta
from fractions import Fraction
from typing import Dict, Iterable, List, Optional

from orderbook import rates

HOME_REGION = "DE"

# Regions where a business customer's VAT id moves the tax to them. Not every
# region in the rates table: outside the EU an export has rules of its own,
# which this module does not implement and refuses rather than guesses.
EU_REGIONS = frozenset(
    "AT BE BG CY CZ DE DK EE ES FI FR GR HR HU IE IT LT LU LV MT NL PL PT RO SE SI SK".split()
)

# What a VAT id looks like, by its two-letter prefix. Shape only: whether the
# number is *registered* is a lookup against VIES that the nightly run does not
# make, so an id that passes here can still be refused by the customer's own
# tax office.
VAT_ID_SHAPES = {
    "AT": r"ATU\d{8}",
    "BE": r"BE[01]\d{9}",
    "DE": r"DE\d{9}",
    "DK": r"DK\d{8}",
    "ES": r"ES[0-9A-Z]\d{7}[0-9A-Z]",
    "FI": r"FI\d{8}",
    "FR": r"FR[0-9A-Z]{2}\d{9}",
    "IE": r"IE\d{7}[A-W][A-I]?",
    "IT": r"IT\d{11}",
    "NL": r"NL\d{9}B\d{2}",
    "PL": r"PL\d{10}",
    "PT": r"PT\d{9}",
    "SE": r"SE\d{12}",
}

# Display only. An invoice is always computed and stored in euro cents; these
# are for the line under the total that tells a customer outside the euro what
# they are roughly paying, and they are updated by hand, monthly.
EXCHANGE = {
    "EUR": Fraction(1),
    "GBP": Fraction(8461, 10000),
    "CHF": Fraction(9312, 10000),
    "SEK": Fraction(114_210, 10000),
    "DKK": Fraction(74_598, 10000),
    "PLN": Fraction(42_870, 10000),
    "CZK": Fraction(251_400, 10000),
    "HUF": Fraction(3_942_100, 10000),
    "NOK": Fraction(117_530, 10000),
}

CURRENCY_BY_REGION = {
    "GB": "GBP",
    "CH": "CHF",
    "SE": "SEK",
    "DK": "DKK",
    "PL": "PLN",
    "CZ": "CZK",
    "HU": "HUF",
    "NO": "NOK",
}


# --- lines -----------------------------------------------------------------


@dataclass(frozen=True)
class Line:
    """One product on an order: how many, and what one costs in cents."""

    sku: str
    qty: int
    unit_cents: int

    def __post_init__(self):
        if self.qty <= 0:
            raise ValueError(f"{self.sku}: a line needs a positive quantity, got {self.qty}")
        if self.unit_cents < 0:
            raise ValueError(f"{self.sku}: a price cannot be negative, got {self.unit_cents}")


@dataclass
class Order:
    """An order as `data/orders.csv` carries it, with its lines attached."""

    order_id: str
    customer: str
    region: str
    discount_pct: int = 0
    lines: List[Line] = field(default_factory=list)
    vat_id: Optional[str] = None
    placed_on: date = date(2026, 11, 6)

    def __post_init__(self):
        if not 0 <= self.discount_pct <= 100:
            raise ValueError(
                f"{self.order_id}: a discount is a percentage, got {self.discount_pct}"
            )
        if self.vat_id is not None and not valid_vat_id(self.vat_id):
            raise ValueError(f"{self.order_id}: {self.vat_id!r} is not a VAT id")


def valid_vat_id(vat_id: str) -> bool:
    """Whether `vat_id` has the shape its country prefix says it should."""
    shape = VAT_ID_SHAPES.get(vat_id[:2])
    return shape is not None and re.fullmatch(shape, vat_id) is not None


def reverse_charge(order: Order) -> bool:
    """Whether the customer, not the seller, accounts for this order's VAT."""
    return (
        order.vat_id is not None
        and order.region != HOME_REGION
        and order.region in EU_REGIONS
        and order.vat_id[:2] == order.region
    )


# --- arithmetic ------------------------------------------------------------


def discount_factor(order: Order) -> Fraction:
    """What is left of a price after the order's discount: 10% off is 9/10."""
    return Fraction(100 - order.discount_pct, 100)


def line_amount(order: Order, line: Line) -> Fraction:
    """A line's amount before tax, with the order's discount applied.

    The discount is applied here, per line, so that a credit note for one line
    refunds what that line actually cost the customer.
    """
    return line.qty * line.unit_cents * discount_factor(order)


def subtotal(order: Order) -> Fraction:
    """The sum of the order's lines, discounted, before tax."""
    return sum((line_amount(order, line) for line in order.lines), Fraction(0))


def tax(order: Order, amount: Fraction) -> Fraction:
    """VAT on `amount` at the rate of the region the order ships to."""
    if reverse_charge(order):
        return Fraction(0)
    return amount * rates.vat_rate(order.region)


def round_cents(amount: Fraction) -> int:
    """Half-up to a whole cent. Called once per invoice, on its total."""
    if amount < 0:
        return -round_cents(-amount)
    whole, rest = divmod(amount, 1)
    return int(whole) + (1 if rest >= Fraction(1, 2) else 0)


# --- invoices --------------------------------------------------------------


@dataclass(frozen=True)
class Invoice:
    order_id: str
    region: str
    net_cents: int
    tax_cents: int
    total_cents: int
    tax_label: str
    reverse_charge: bool = False

    def as_row(self) -> List[str]:
        return [
            self.order_id,
            self.region,
            str(self.net_cents),
            str(self.tax_cents),
            str(self.total_cents),
        ]


def invoice_total(order: Order) -> int:
    """What the customer pays for `order`, in cents: net, discount, VAT.

    Rounded once, here, and nowhere before.
    """
    net = subtotal(order) * discount_factor(order)
    return round_cents(net + tax(order, net))


def make_invoice(order: Order) -> Invoice:
    """The invoice for `order`, with its parts rounded so they add up.

    Net and total are rounded on their own and tax is what is left between
    them, so `net_cents + tax_cents == total_cents` on every invoice even when
    rounding each part alone would leave a cent over.
    """
    if not order.lines:
        raise ValueError(f"{order.order_id}: an order with no lines has no invoice")
    total = invoice_total(order)
    net = round_cents(subtotal(order))
    return Invoice(
        order_id=order.order_id,
        region=order.region,
        net_cents=net,
        tax_cents=total - net,
        total_cents=total,
        tax_label=rates.vat_label(order.region),
        reverse_charge=reverse_charge(order),
    )


class InvoiceNumbers:
    """Consecutive invoice numbers, restarting each year: `2026-000141`.

    Gapless by law in most of the regions above, so a number is only taken
    once an invoice has been made; a run that stops halfway leaves the next
    number where the last invoice left it.
    """

    def __init__(self, year: int, last: int = 0):
        self.year = year
        self.last = last

    def take(self, on: date) -> str:
        if on.year != self.year:
            self.year, self.last = on.year, 0
        self.last += 1
        return f"{self.year}-{self.last:06d}"


def due_date(placed_on: date, terms_days: int = 30) -> date:
    """When payment is due: `terms_days` after the order, moved off a weekend."""
    due = placed_on + timedelta(days=terms_days)
    while due.weekday() >= 5:
        due += timedelta(days=1)
    return due


# --- credit notes ----------------------------------------------------------


@dataclass(frozen=True)
class CreditNote:
    order_id: str
    sku: str
    qty: int
    refund_cents: int


def credit_note(order: Order, sku: str, qty: Optional[int] = None) -> CreditNote:
    """A refund for `qty` of `sku` on `order` (all of them when `qty` is None).

    A refund is the line's own discounted amount plus its VAT, pro rata, so a
    customer who returns everything gets back what they paid to the cent.
    """
    matching = [line for line in order.lines if line.sku == sku]
    if not matching:
        raise KeyError(f"{order.order_id} has no line for {sku}")
    line = matching[0]
    qty = line.qty if qty is None else qty
    if not 0 < qty <= line.qty:
        raise ValueError(f"{order.order_id}: cannot refund {qty} of {line.qty} {sku}")
    amount = line_amount(order, line) * Fraction(qty, line.qty)
    return CreditNote(
        order_id=order.order_id,
        sku=sku,
        qty=qty,
        refund_cents=round_cents(amount + tax(order, amount)),
    )


# --- formatting ------------------------------------------------------------


def format_cents(cents: int, symbol: str = "€") -> str:
    """`123456` as `1,234.56 €`. Negative amounts keep their sign in front."""
    sign = "-" if cents < 0 else ""
    whole, rest = divmod(abs(cents), 100)
    return f"{sign}{whole:,}.{rest:02d} {symbol}"


def convert(cents: int, currency: str) -> int:
    """Euro cents as the minor unit of `currency`, for display only."""
    try:
        rate = EXCHANGE[currency]
    except KeyError:
        raise ValueError(f"no exchange rate for {currency}") from None
    return round_cents(cents * rate)


def describe(invoice: Invoice) -> str:
    """The lines the nightly run would log for an invoice, if asked to."""
    return "\n".join(
        [
            f"invoice {invoice.order_id} ({invoice.region})",
            f"  net    {format_cents(invoice.net_cents)}",
            f"  {invoice.tax_label:<6} {format_cents(invoice.tax_cents)}",
            f"  total  {format_cents(invoice.total_cents)}",
        ]
    )


def render(order: Order, invoice: Invoice, number: str, terms_days: int = 30) -> str:
    """The invoice as the customer reads it: lines, totals, terms, notes."""
    out = [
        f"INVOICE {number}",
        f"order {order.order_id} · {order.customer} · ships to {order.region}",
        f"date {order.placed_on.isoformat()} · due {due_date(order.placed_on, terms_days).isoformat()}",
        "",
    ]
    for line in order.lines:
        amount = round_cents(line_amount(order, line))
        out.append(
            f"  {line.sku:<12} {line.qty:>3} × {format_cents(line.unit_cents):>12}"
            f"  {format_cents(amount):>12}"
        )
    if order.discount_pct:
        out.append(f"  (prices above include a {order.discount_pct}% discount)")
    out += [
        "",
        f"  {'net':<20} {format_cents(invoice.net_cents):>12}",
        f"  {invoice.tax_label:<20} {format_cents(invoice.tax_cents):>12}",
        f"  {'total':<20} {format_cents(invoice.total_cents):>12}",
    ]
    currency = CURRENCY_BY_REGION.get(order.region)
    if currency:
        approx = convert(invoice.total_cents, currency)
        out.append(f"  {'approx.':<20} {format_cents(approx, currency):>12}")
    if invoice.reverse_charge:
        out += ["", f"  Reverse charge: VAT to be accounted for by the customer ({order.vat_id})."]
    return "\n".join(out)


# --- batches ---------------------------------------------------------------


@dataclass
class RegionTotals:
    invoices: int = 0
    net_cents: int = 0
    tax_cents: int = 0
    total_cents: int = 0


def summarise(invoices: Iterable[Invoice]) -> Dict[str, RegionTotals]:
    """Totals by region, for the line the ledger is checked against."""
    totals: Dict[str, RegionTotals] = defaultdict(RegionTotals)
    for invoice in invoices:
        t = totals[invoice.region]
        t.invoices += 1
        t.net_cents += invoice.net_cents
        t.tax_cents += invoice.tax_cents
        t.total_cents += invoice.total_cents
    return dict(sorted(totals.items()))


def check_against_ledger(totals: Dict[str, RegionTotals], ledger: Dict[str, int]) -> List[str]:
    """Every region whose invoiced total is not what the ledger booked.

    One line per region, with both figures, so the nightly log says which
    region to look at rather than that something, somewhere, is a cent out.
    """
    problems = []
    for region in sorted(set(totals) | set(ledger)):
        invoiced = totals.get(region, RegionTotals()).total_cents
        booked = ledger.get(region, 0)
        if invoiced != booked:
            problems.append(
                f"{region}: invoiced {format_cents(invoiced)}, ledger {format_cents(booked)}"
            )
    return problems


# --- reminders -------------------------------------------------------------

# Days after the due date at which each reminder goes out, and the fee it adds.
# The first is free; a customer who forgot once should not pay for it.
REMINDERS = [
    (7, 0),
    (21, 500),
    (35, 1000),
]

# Statutory interest on late payment between businesses, as a yearly rate over
# the reference rate. Consumers are charged the fees above and nothing else.
LATE_INTEREST_OVER_REFERENCE = Fraction(9, 100)
REFERENCE_RATE = Fraction(227, 10000)


@dataclass(frozen=True)
class Reminder:
    order_id: str
    level: int
    fee_cents: int
    interest_cents: int
    owed_cents: int


def reminder(order: Order, invoice: Invoice, today: date, paid_cents: int = 0) -> Optional[Reminder]:
    """The reminder due for `invoice` on `today`, or None if none is due yet.

    The level is the last threshold passed, so a run that skipped a week sends
    the right level rather than the next one. Interest accrues by the day from
    the due date, on what is still owed, for business customers only.
    """
    owed = invoice.total_cents - paid_cents
    if owed <= 0:
        return None
    late_by = (today - due_date(order.placed_on)).days
    level = sum(1 for days, _ in REMINDERS if late_by >= days)
    if level == 0:
        return None
    fee = sum(fee for _, fee in REMINDERS[:level])
    interest = 0
    if order.vat_id is not None:
        yearly = LATE_INTEREST_OVER_REFERENCE + REFERENCE_RATE
        interest = round_cents(owed * yearly * Fraction(late_by, 365))
    return Reminder(
        order_id=order.order_id,
        level=level,
        fee_cents=fee,
        interest_cents=interest,
        owed_cents=owed + fee + interest,
    )


# --- the accountant's export -----------------------------------------------

EXPORT_COLUMNS = [
    "number",
    "order_id",
    "date",
    "region",
    "net_cents",
    "tax_cents",
    "total_cents",
    "tax_label",
    "reverse_charge",
]


def export_rows(pairs: Iterable[tuple]) -> List[List[str]]:
    """One row per (number, order, invoice), in the columns above.

    The accountant's software reads cents as integers and the reverse-charge
    flag as `1` or `0`; anything else in that column fails their import with
    an error that names the row and not the column, so it is spelled out here.
    """
    rows = [list(EXPORT_COLUMNS)]
    for number, order, invoice in pairs:
        rows.append(
            [
                number,
                order.order_id,
                order.placed_on.isoformat(),
                invoice.region,
                str(invoice.net_cents),
                str(invoice.tax_cents),
                str(invoice.total_cents),
                invoice.tax_label,
                "1" if invoice.reverse_charge else "0",
            ]
        )
    return rows


def split_large_order(order: Order, limit_cents: int) -> List[Order]:
    """`order` as several orders whose subtotals each stay under `limit_cents`.

    Some regions require an itemised invoice above a threshold that a customer
    buying for a whole team crosses; splitting by line keeps each part simple.
    A single line over the limit is kept whole: splitting a line would change
    its unit price's rounding, which is the one thing this module never does.
    """
    parts: List[Order] = []
    current: List[Line] = []
    running = Fraction(0)
    for line in order.lines:
        amount = line_amount(order, line)
        if current and running + amount > limit_cents:
            parts.append(_part(order, current, len(parts) + 1))
            current, running = [], Fraction(0)
        current.append(line)
        running += amount
    if current:
        parts.append(_part(order, current, len(parts) + 1))
    return parts


def _part(order: Order, lines: List[Line], n: int) -> Order:
    return Order(
        order_id=f"{order.order_id}/{n}",
        customer=order.customer,
        region=order.region,
        discount_pct=order.discount_pct,
        lines=list(lines),
        vat_id=order.vat_id,
        placed_on=order.placed_on,
    )
