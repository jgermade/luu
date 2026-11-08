"""VAT by region, as a fraction, and what each region's invoice says about it.

A region is the two-letter code an order ships to. An order to a region that is
not in this table cannot be invoiced: `vat_rate` raises rather than guessing,
because an invoice with the wrong tax on it is worse than no invoice.
"""

from fractions import Fraction

# region: (rate, the name printed on the invoice's tax line)
REGION_RATES = {
    "AT": (Fraction(20, 100), "USt."),
    "BE": (Fraction(21, 100), "BTW"),
    "BG": (Fraction(20, 100), "ДДС"),
    "CY": (Fraction(19, 100), "ΦΠΑ"),
    "CZ": (Fraction(21, 100), "DPH"),
    "DE": (Fraction(19, 100), "MwSt."),
    "DK": (Fraction(25, 100), "moms"),
    "EE": (Fraction(22, 100), "km"),
    "ES": (Fraction(21, 100), "IVA"),
    "FI": (Fraction(24, 100), "ALV"),
    "FR": (Fraction(20, 100), "TVA"),
    "GR": (Fraction(24, 100), "ΦΠΑ"),
    "HR": (Fraction(25, 100), "PDV"),
    "HU": (Fraction(27, 100), "ÁFA"),
    "IE": (Fraction(23, 100), "VAT"),
    "IT": (Fraction(22, 100), "IVA"),
    "LT": (Fraction(21, 100), "PVM"),
    "LU": (Fraction(17, 100), "TVA"),
    "LV": (Fraction(21, 100), "PVN"),
    "MT": (Fraction(18, 100), "VAT"),
    "NL": (Fraction(21, 100), "BTW"),
    "PL": (Fraction(23, 100), "VAT"),
    "PT": (Fraction(23, 100), "IVA"),
    "RO": (Fraction(19, 100), "TVA"),
    "SE": (Fraction(25, 100), "moms"),
    "SI": (Fraction(22, 100), "DDV"),
    "SK": (Fraction(20, 100), "DPH"),
    "NO": (Fraction(25, 100), "MVA"),
    "CH": (Fraction(81, 1000), "MWST"),
    "GB": (Fraction(20, 100), "VAT"),
}


# Reduced rates by category, where a region has one. Nothing in the shop sells
# in these categories yet (every SKU is merchandise, at the standard rate), but
# the posters were nearly classed as printed matter once, and the table was
# written then. A category missing for a region means the standard rate.
REDUCED_RATES = {
    "AT": {"books": Fraction(10, 100), "food": Fraction(10, 100)},
    "BE": {"books": Fraction(6, 100), "food": Fraction(6, 100)},
    "CZ": {"books": Fraction(0, 100), "food": Fraction(12, 100)},
    "DE": {"books": Fraction(7, 100), "food": Fraction(7, 100)},
    "DK": {},
    "ES": {"books": Fraction(4, 100), "food": Fraction(10, 100)},
    "FI": {"books": Fraction(14, 100), "food": Fraction(14, 100)},
    "FR": {"books": Fraction(55, 1000), "food": Fraction(55, 1000)},
    "GR": {"books": Fraction(6, 100), "food": Fraction(13, 100)},
    "HU": {"books": Fraction(5, 100), "food": Fraction(18, 100)},
    "IE": {"books": Fraction(0, 100), "food": Fraction(0, 100)},
    "IT": {"books": Fraction(4, 100), "food": Fraction(10, 100)},
    "NL": {"books": Fraction(9, 100), "food": Fraction(9, 100)},
    "PL": {"books": Fraction(5, 100), "food": Fraction(5, 100)},
    "PT": {"books": Fraction(6, 100), "food": Fraction(6, 100)},
    "SE": {"books": Fraction(6, 100), "food": Fraction(12, 100)},
    "NO": {"books": Fraction(0, 100), "food": Fraction(15, 100)},
    "CH": {"books": Fraction(26, 1000), "food": Fraction(26, 1000)},
    "GB": {"books": Fraction(0, 100), "food": Fraction(0, 100)},
}


class UnknownRegion(KeyError):
    """An order ships somewhere this table has no rate for."""


def vat_rate(region):
    """The VAT rate for `region`, as a Fraction."""
    try:
        return REGION_RATES[region][0]
    except KeyError:
        raise UnknownRegion(region) from None


def vat_label(region):
    """What the invoice's tax line is called in `region`."""
    try:
        return REGION_RATES[region][1]
    except KeyError:
        raise UnknownRegion(region) from None


def rate_for(region, category=None):
    """The rate for `category` in `region`: its reduced rate, or the standard."""
    standard = vat_rate(region)
    if category is None:
        return standard
    return REDUCED_RATES.get(region, {}).get(category, standard)


def regions():
    """Every region an order can be invoiced to, sorted."""
    return sorted(REGION_RATES)
