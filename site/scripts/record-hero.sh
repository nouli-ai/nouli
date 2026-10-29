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

TRANSPORT='transport: How should I travel to the meeting? [Take the train, Take a taxi]'
APPROVAL='approval: May I book travel costing more than 50 USD without approval?'
ON_TIME='on_time: Will the train get me to the meeting on time?'
TRANSPORT_NOW='transport: How should I travel to the meeting now? [Take the train, Take a taxi]'
BOOKING2='booking: What should I do next? [Wait for more approval, Book the 40 USD train, Book the 70 USD taxi]'
BOOKING='booking: What should I do next? [Request approval for the 70 USD taxi, Book the 40 USD train, Book the taxi without approval]'
PLAN_FACTS='Attendance must be in person. Choose a next step that respects both the latest travel information and the spending rule.'
APPROVED='My manager has approved spending up to 80 USD on this trip.'

done_ "$OUT/travel-1.json" || n new hero-travel 'The train costs 40 USD and arrives 30 minutes before the meeting. A taxi costs 70 USD and also arrives in time. Prefer the cheaper option that arrives on time.' -q "$TRANSPORT" > "$OUT/travel-1.json"
done_ "$OUT/budget-1.json" || n new hero-budget 'The travel budget is 50 USD. Spending more requires approval before booking. No exception has been approved.' -q "$APPROVAL" > "$OUT/budget-1.json"
T1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["result"]["report"]["turn"])' "$OUT/travel-1.json" 2>/dev/null || true)

for time in cancelled late ontime; do
  case $time in
    cancelled) facts='The train has been cancelled. The taxi is still available at 70 USD.' ;;
    late) facts='The train is running 10 minutes late but will still arrive 20 minutes before the meeting.' ;;
    ontime) facts='The train is running on time.' ;;
  esac
  if [ "$time" = cancelled ]; then
    done_ "$OUT/travel-2-$time.json" || n add hero-travel "$facts" -q "$ON_TIME" -q "$TRANSPORT_NOW" > "$OUT/travel-2-$time.json"
    travel=hero-travel
  else
    done_ "$OUT/travel-2-$time.json" || n new "hero-travel-$time" --from "hero-travel@$T1" "$facts" -q "$ON_TIME" -q "$TRANSPORT_NOW" > "$OUT/travel-2-$time.json"
    travel=hero-travel-$time
  fi
  for srcs in both travel; do
    plan="hero-plan-$time-$srcs"
    if [ "$srcs" = both ]; then
      done_ "$OUT/plan-1-$time-$srcs.json" || n new "$plan" --from "$travel" --from hero-budget "$PLAN_FACTS" -q "$BOOKING" > "$OUT/plan-1-$time-$srcs.json"
    else
      done_ "$OUT/plan-1-$time-$srcs.json" || { n new "$plan" --from "$travel" > /dev/null; n add "$plan" "$PLAN_FACTS" -q "$BOOKING" > "$OUT/plan-1-$time-$srcs.json"; }
    fi
    done_ "$OUT/plan-2-$time-$srcs.json" || n add "$plan" "$APPROVED" -q "$BOOKING2" > "$OUT/plan-2-$time-$srcs.json"
  done
done
done_ "$OUT/plan-1-budget.json" || { n new hero-plan-budget --from hero-budget > /dev/null; n add hero-plan-budget "$PLAN_FACTS" -q "$BOOKING" > "$OUT/plan-1-budget.json"; }
done_ "$OUT/plan-2-budget.json" || n add hero-plan-budget "$APPROVED" -q "$BOOKING2" > "$OUT/plan-2-budget.json"
echo recorded
