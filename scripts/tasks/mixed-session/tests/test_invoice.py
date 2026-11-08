import io
import unittest
from datetime import date, datetime
from fractions import Fraction

from orderbook import nightly, rates
from orderbook.invoice import (
    InvoiceNumbers,
    Line,
    Order,
    credit_note,
    due_date,
    format_cents,
    invoice_total,
    line_amount,
    make_invoice,
    round_cents,
    subtotal,
)


def order(region="DE", discount_pct=0, *lines, vat_id=None):
    return Order(
        order_id="T-1",
        customer="Test Customer",
        region=region,
        discount_pct=discount_pct,
        lines=list(lines) or [Line("MUG-01", 1, 1000)],
        vat_id=vat_id,
    )


class Rounding(unittest.TestCase):
    def test_half_a_cent_rounds_up(self):
        self.assertEqual(round_cents(Fraction(201, 2)), 101)

    def test_under_half_a_cent_rounds_down(self):
        self.assertEqual(round_cents(Fraction(1004, 10)), 100)

    def test_negative_amounts_round_away_from_zero_at_half(self):
        self.assertEqual(round_cents(Fraction(-201, 2)), -101)


class Lines(unittest.TestCase):
    def test_a_line_is_quantity_times_price(self):
        o = order("DE", 0, Line("TEE-M", 3, 1990))
        self.assertEqual(line_amount(o, o.lines[0]), 5970)

    def test_the_discount_is_applied_to_each_line(self):
        o = order("DE", 10, Line("TEE-M", 3, 1990))
        self.assertEqual(line_amount(o, o.lines[0]), Fraction(5373))

    def test_a_subtotal_sums_the_discounted_lines(self):
        o = order("DE", 20, Line("MUG-01", 1, 1000), Line("CAP-01", 2, 1500))
        self.assertEqual(subtotal(o), 3200)


class Totals(unittest.TestCase):
    def test_invoice_total_without_a_discount(self):
        o = order("DE", 0, Line("MUG-01", 2, 1290))
        self.assertEqual(invoice_total(o), 3070)  # 2580 + 19% = 3070.2

    def test_invoice_total_applies_the_discount_once(self):
        o = order("ES", 10, Line("BAG-TOTE", 2, 2500))
        # 5000 less 10% is 4500; IVA at 21% is 945.
        self.assertEqual(invoice_total(o), 5445)

    def test_net_and_tax_add_up_to_the_total(self):
        o = order("HU", 0, Line("NOTEBOOK", 3, 1250), Line("STICKERS", 7, 450))
        inv = make_invoice(o)
        self.assertEqual(inv.net_cents + inv.tax_cents, inv.total_cents)

    def test_an_unknown_region_is_refused(self):
        with self.assertRaises(rates.UnknownRegion):
            invoice_total(order("XX"))


class ReverseCharge(unittest.TestCase):
    def test_an_eu_business_elsewhere_is_invoiced_without_vat(self):
        o = order("FR", 0, Line("MUG-01", 2, 1290), vat_id="FR12345678901")
        self.assertEqual(invoice_total(o), 2580)

    def test_a_vat_id_from_another_region_does_not_move_the_tax(self):
        o = order("FR", 0, Line("MUG-01", 2, 1290), vat_id="DE123456789")
        self.assertEqual(invoice_total(o), 3096)


class Terms(unittest.TestCase):
    def test_a_due_date_on_a_sunday_moves_to_monday(self):
        self.assertEqual(due_date(date(2026, 11, 6)), date(2026, 12, 7))

    def test_invoice_numbers_restart_each_year(self):
        numbers = InvoiceNumbers(2026, last=140)
        self.assertEqual(numbers.take(date(2026, 11, 7)), "2026-000141")
        self.assertEqual(numbers.take(date(2027, 1, 2)), "2027-000001")


class CreditNotes(unittest.TestCase):
    def test_returning_everything_refunds_the_line_with_its_tax(self):
        o = order("FR", 0, Line("HOODIE-M", 1, 4990))
        self.assertEqual(credit_note(o, "HOODIE-M").refund_cents, 5988)

    def test_a_partial_return_is_pro_rata(self):
        o = order("FR", 0, Line("STICKERS", 4, 450))
        self.assertEqual(credit_note(o, "STICKERS", 1).refund_cents, 540)


class Formatting(unittest.TestCase):
    def test_thousands_and_cents(self):
        self.assertEqual(format_cents(123456), "1,234.56 €")

    def test_a_negative_amount_keeps_its_sign_in_front(self):
        self.assertEqual(format_cents(-5), "-0.05 €")


class Nightly(unittest.TestCase):
    def test_a_run_stops_at_the_first_order_it_cannot_invoice(self):
        orders = [order("DE"), order("XX"), order("FR")]
        log = io.StringIO()
        code = nightly.run(log, clock=nightly.ticking(datetime(2026, 1, 1)), orders=orders)
        self.assertEqual(code, 1)
        self.assertIn("stopped after 1 of 3 orders", log.getvalue())


if __name__ == "__main__":
    unittest.main()
