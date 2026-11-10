#!/usr/bin/env bash
# The release dry run's Maven Central step (MOBILE-1e): upload the signed
# bundle with the plugin's publishToMavenCentral as a USER_MANAGED
# deployment (never released), wait until the Central Portal validates it,
# then drop it through the Portal API.
#
#   MAVEN_CENTRAL_USERNAME=... MAVEN_CENTRAL_PASSWORD=... scripts/central-dry-run.sh
#
# The Gradle signing and credential properties come from the environment
# (ORG_GRADLE_PROJECT_*), as in release.yml. Nothing here prints a
# credential. A deployment that isn't USER_MANAGED, or that reaches
# PUBLISHING or PUBLISHED, fails the script loudly.
set -euo pipefail

: "${MAVEN_CENTRAL_USERNAME:?}" "${MAVEN_CENTRAL_PASSWORD:?}"
API=https://central.sonatype.com/api/v1/publisher
AUTH="Authorization: Bearer $(printf '%s:%s' "$MAVEN_CENTRAL_USERNAME" "$MAVEN_CENTRAL_PASSWORD" | base64 | tr -d '\n')"
LOG="$(mktemp)"
trap 'rm -f "$LOG" "$LOG.body"' EXIT

state_of() {   # deployment id -> its deploymentState (or the HTTP code)
  local code
  code=$(curl -sS -o "$LOG.body" -w '%{http_code}' -X POST -H "$AUTH" "$API/status?id=$1")
  if [ "$code" != 200 ]; then echo "HTTP_$code"; return; fi
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["deploymentState"])' "$LOG.body"
}

echo "==> publishToMavenCentral (USER_MANAGED: upload and validate, never release)"
status=0
./gradlew --no-daemon --stacktrace publishToMavenCentral \
  -PmavenCentralAutomaticPublishing=false 2>&1 | tee "$LOG" || status=$?

line=$(grep -m1 "Uploaded bundle to Central Portal as" "$LOG" || true)
if [ -z "$line" ]; then
  echo "no upload happened (gradle exit $status)" >&2
  exit 1
fi
ID=$(sed -n 's/.*deployment id: \([0-9a-fA-F-]*\).*/\1/p' <<<"$line")
echo "==> $line"
case "$line" in
  *"as USER_MANAGED"*) ;;
  *) echo "the upload was not USER_MANAGED: it may release; check the Portal now (deployment $ID)" >&2; exit 1 ;;
esac

# The plugin waits for VALIDATED; if it gave up early, keep polling.
for _ in $(seq 1 60); do
  STATE=$(state_of "$ID")
  case "$STATE" in VALIDATED|FAILED|PUBLISHING|PUBLISHED|HTTP_*) break ;; esac
  sleep 10
done
echo "==> deployment $ID is $STATE"
case "$STATE" in
  PUBLISHING|PUBLISHED) echo "deployment $ID is being published: this was not a dry run" >&2; exit 1 ;;
  VALIDATED|FAILED) ;;
  *) echo "deployment $ID never finished validating ($STATE); drop it in the Portal" >&2; exit 1 ;;
esac
if [ "$STATE" = FAILED ]; then
  python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])).get("errors"), indent=1))' "$LOG.body" || true
fi

echo "==> dropping deployment $ID"
code=$(curl -sS -o "$LOG.body" -w '%{http_code}' -X DELETE -H "$AUTH" "$API/deployment/$ID")
if [ "$code" != 204 ]; then
  echo "drop failed: HTTP $code: $(cat "$LOG.body")" >&2
  exit 1
fi
after=$(state_of "$ID")
echo "==> dropped (HTTP 204); the status call now says: $after"
[ "$STATE" = VALIDATED ] || { echo "the deployment failed validation (dropped)" >&2; exit 1; }
[ "$status" = 0 ] || { echo "gradle exited $status" >&2; exit 1; }
echo "==> dry run: the signed bundle validated on the Central Portal and was dropped, unreleased"
