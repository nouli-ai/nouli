#!/bin/sh
# Record the home-page decision graph and its what-if and blend variants with real model calls.
#
#   sh scripts/record-hero.sh OUTPUT_DIR
#
# Run from a directory whose .env holds the provider key. Calls use your provider account.
# Rerunning skips calls whose output file already holds a finished turn.
set -eu
NOULI=${NOULI:-nouli}
OUT=${1:?usage: record-hero.sh OUTPUT_DIR}
STORE=$OUT/store
mkdir -p "$OUT"
n() { "$NOULI" --store "$STORE" --json "$@"; }
done_() { python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["result"]["kind"]=="turn" else 1)' "$1" 2>/dev/null; }

TRANSPORT='[{"id":"transport","prompt":"How should I travel to the meeting?","answer_space":{"type":"choice","options":[{"id":"train","description":"Take the train"},{"id":"taxi","description":"Take a taxi"}]}}]'
APPROVAL='[{"id":"approval","prompt":"May I book travel costing more than 50 USD without approval?","answer_space":{"type":"noul"}}]'
TURN2='[{"id":"on_time","prompt":"Will the train get me to the meeting on time?","answer_space":{"type":"noul"}},{"id":"transport","prompt":"How should I travel to the meeting now?","answer_space":{"type":"choice","options":[{"id":"train","description":"Take the train"},{"id":"taxi","description":"Take a taxi"}]}}]'
BOOKING2='[{"id":"booking","prompt":"What should I do next?","answer_space":{"type":"choice","options":[{"id":"wait","description":"Wait for more approval"},{"id":"train","description":"Book the 40 USD train"},{"id":"taxi","description":"Book the 70 USD taxi"}]}}]'
BOOKING='[{"id":"booking","prompt":"What should I do next?","answer_space":{"type":"choice","options":[{"id":"approval","description":"Request approval for the 70 USD taxi"},{"id":"train","description":"Book the 40 USD train"},{"id":"taxi","description":"Book the taxi without approval"}]}}]'

done_ "$OUT/travel-1.json" || n --name hero-travel -s 'The train costs 40 USD and arrives 30 minutes before the meeting. A taxi costs 70 USD and also arrives in time. Prefer the cheaper option that arrives on time.' -q "$TRANSPORT" > "$OUT/travel-1.json"
done_ "$OUT/budget-1.json" || n --name hero-budget -s 'The travel budget is 50 USD. Spending more requires approval before booking. No exception has been approved.' -q "$APPROVAL" > "$OUT/budget-1.json"
T1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["result"]["report"]["turn"])' "$OUT/travel-1.json" 2>/dev/null || true)

for time in cancelled late ontime; do
  case $time in
    cancelled) facts='The train has been cancelled. The taxi is still available at 70 USD.' ;;
    late) facts='The train is running 10 minutes late but will still arrive 20 minutes before the meeting.' ;;
    ontime) facts='The train is running on time.' ;;
  esac
  if [ "$time" = cancelled ]; then
    done_ "$OUT/travel-2-$time.json" || n --session hero-travel -s "$facts" -q "$TURN2" > "$OUT/travel-2-$time.json"
    travel=hero-travel
  else
    done_ "$OUT/travel-2-$time.json" || n --from "hero-travel@$T1" --name "hero-travel-$time" -s "$facts" -q "$TURN2" > "$OUT/travel-2-$time.json"
    travel=hero-travel-$time
  fi
  for srcs in both travel; do
    if [ "$srcs" = both ]; then from="--from $travel --from hero-budget"; else from="--from $travel"; fi
    # shellcheck disable=SC2086
    done_ "$OUT/plan-1-$time-$srcs.json" || n $from --name "hero-plan-$time-$srcs" -s 'Attendance must be in person. Choose a next step that respects both the latest travel information and the spending rule.' -q "$BOOKING" > "$OUT/plan-1-$time-$srcs.json"
    done_ "$OUT/plan-2-$time-$srcs.json" || n --session "hero-plan-$time-$srcs" -s 'My manager has approved spending up to 80 USD on this trip.' -q "$BOOKING2" > "$OUT/plan-2-$time-$srcs.json"
  done
done
done_ "$OUT/plan-1-budget.json" || n --from hero-budget --name hero-plan-budget -s 'Attendance must be in person. Choose a next step that respects both the latest travel information and the spending rule.' -q "$BOOKING" > "$OUT/plan-1-budget.json"
done_ "$OUT/plan-2-budget.json" || n --session hero-plan-budget -s 'My manager has approved spending up to 80 USD on this trip.' -q "$BOOKING2" > "$OUT/plan-2-budget.json"
echo recorded
