# Usage statistics, like `omp stats`

Huan, 2026-11-16: "I'd like Sandhill to have something like omp stats
results."
Status: approved and built 2026-11-16 with Huan's answers: Sandhill
records the replies of the conversations it runs (B), from now on; a
Welcome page "Usage" beside Agents; the estimated cost shown. Built as
(B) describes, the price list was not needed: the costs are the agents'
own (Claude Code's status line, pi's and Oh My Pi's reports); Codex
gives none and shows "–". Antigravity and Posit Assistant report no
tokens to Sandhill and are not counted.

## What `omp stats` shows

Read from omp 18.21 itself (its summary and dashboard), for a chosen
period:

- Overall: requests (and how many failed), error rate, tokens (input,
  output, total), cache rate, cost, average duration, time to the first
  word, words per second.
- By model: requests, cost, cache rate, what the cache saved.
- By folder (project): requests and cost.
- Over time: a day-by-day series and "last N weeks" activity.

Costs are "API-equivalent estimates": what the tokens would cost at the
providers' list prices, also on a subscription.

## Where the numbers come from

Each agent keeps its conversations on the device, with every reply's
token counts and time:

| Agent | Its files | Per reply |
|---|---|---|
| Claude Code | `projects/<folder>/<conversation>.jsonl`, in the user's folder and Sandhill's added accounts | model, input, output, cache read, cache written, time, folder |
| Codex | `sessions/<year>/<month>/<day>/rollout-….jsonl` | model, input, cached input, output, reasoning, time, folder |
| Oh My Pi, pi | `agent/sessions/…/*.jsonl` | provider, model, input, output, cache read/written, cost, time, folder |
| Antigravity, OpenScience, Posit Assistant | no usable record | — |

**Ask** which to use:

- (A) The daemon reads these files on each device, read only: all use,
  also outside Sandhill, back as far as the files go. It never opens
  logins or keys; it only reads the conversation records. (Reading these
  folders is new: until now Sandhill only links them for accounts.)
- (B) Sandhill records each reply's tokens itself from the conversations
  it runs: only use inside Sandhill, from the day this is built.

Requests that failed and the time to the first word are in none of these
files except omp's; those two figures show for omp only, or are left out.

## Cost

Claude Code's and Codex's files have no prices. Sandhill keeps a small
price list for the models these agents use (list prices per million
tokens, with cache prices), shipped with the app and dated; a model not
on it shows "–", never a guess. omp's own costs are used as they are.

## Where it shows

**Ask**: a new Welcome page, **Usage**, beside Agents (one device at a
time, as Agents and Resources are), or a section of the status bar's
quota panel. Proposed page, for the chosen device and period
(7 days, 30 days, 90 days, a year):

```
Usage · This Mac                          [7d] [30d] [90d] [1y]   Refresh
┌────────────┬────────────┬────────────┬────────────┬────────────┐
│ Requests   │ Tokens     │ Cache rate │ Cost (est.)│ Active days│
│ 4,812      │ 312 M      │ 91%        │ $1,240     │ 26 of 30   │
└────────────┴────────────┴────────────┴────────────┴────────────┘
Per day   ▁▂▅▇▆▃▁▂▆█▇▅▂▁ …   (bars by agent; hover: that day's figures)
By agent and model                     requests  tokens  cache  cost
  Claude Code · claude-opus-…          2,140     180 M   93%    $820
  Codex · gpt-6.1-sol                  1,960     110 M   88%    $390
  Oh My Pi · …                           712      22 M   85%     $30
By project                             requests  tokens         cost
  sandhill                             3,100     240 M          $960
  scrna-atlas                          1,050      60 M          $250
```

Tokens split into input, output and cache on hover. Projects are the
conversations' folders, named as the Projects page names them.

## How it works

- Daemon: a `usage` module reads the files (A) or Sandhill's own records
  (B) into one list of replies (time, agent, account, model, folder,
  tokens, cost), keeps a small cache keyed by each file's size and time,
  so a refresh reads only what changed, and answers `Call::UsageStats
  {since}` with the totals, the per-day series and the two tables. The
  agents' formats are each parsed by a small function with a test on a
  sample of that format.
- App: the page draws the cards, the bars (by day, coloured by agent)
  and the tables, asks when the page opens and on Refresh, never on a
  timer.
- Nothing leaves the device; nothing is sent to any provider.

## Checks

A daemon test per format (sample files, expected totals), a test of the
totals and the per-day series, and an integration test of the call. The
page looked at in a test copy of the app; on Huan's Mac the numbers
checked once against `omp stats --summary` for omp's part.
