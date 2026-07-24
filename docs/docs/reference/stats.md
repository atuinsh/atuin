# stats

Atuin can also calculate stats based on your history - this is currently a
little basic, but more features to come.

The optional `[PERIOD]...` argument selects the time range. Leave it blank (or
pass `all`) for your entire history. Built-in shortcuts and free-form dates are
described below.

## Period shortcuts

These single-word values are recognized by `atuin stats` (matched after joining
the period arguments into one string):

| Period | Range |
|--------|--------|
| *(empty)* / `all` | Entire history |
| `today` | From local midnight today to midnight tomorrow |
| `week` | Rolling last 7 days, ending at local midnight today |
| `month` | Rolling last 31 days, ending at local midnight today |
| `year` | Rolling last 365 days, ending at local midnight today |

```
$ atuin stats today
$ atuin stats week
$ atuin stats month
$ atuin stats year
```

`week` / `month` / `year` are rolling windows, not calendar weeks, months, or
years. For example, `month` is the previous 31 days before today's midnight —
not "the current calendar month."

## Single-day stats (parsed dates)

Any other period string is parsed as a start time with `interim`. Stats cover
**24 hours from that start** (not the whole named calendar unit). Certain
formats rely on the dialect option in your
[configuration](../configuration/config.md#dialect) to differentiate day from month.
Refer to [the module's documentation](https://docs.rs/interim/latest/interim/#supported-formats) for more details on the supported date formats.

```console
$ atuin stats last friday

+---------------------+------------+
| Statistic           | Value      |
+---------------------+------------+
| Most used command   | git status |
+---------------------+------------+
| Commands ran        |        450 |
+---------------------+------------+
| Unique commands ran |        213 |
+---------------------+------------+

# A few more examples:
$ atuin stats 2018-04-01
$ atuin stats April 1
$ atuin stats 01/04/22
$ atuin stats last thursday 3pm  # between last thursday 3:00pm and the following friday 3:00pm
```

!!! note
    A bare month name such as `june` is treated as a single date (typically the
    1st of June in the current/previous year per `interim`), then expanded to a
    24-hour window — **not** the whole of June. Prefer `month` for a rolling
    31-day window, or pass an explicit date range start.

## Full history stats

```console
$ atuin stats
# or
$ atuin stats all

+---------------------+-------+
| Statistic           | Value |
+---------------------+-------+
| Most used command   |    ls |
+---------------------+-------+
| Commands ran        |  8190 |
+---------------------+-------+
| Unique commands ran |  2996 |
+---------------------+-------+
```
