#!/usr/bin/env bash
# Integration test for channel hooks (bd-2km)
# Usage: ./scripts/test-hooks.sh
set -euo pipefail

BUS="${BUS:-cargo run --release --bin bus --}"
BOTBUS_DATA_DIR=$(mktemp -d)
export BOTBUS_DATA_DIR
export BOTBUS_AGENT="${BOTBUS_AGENT:-test-agent}"

cleanup() {
    rm -rf "$BOTBUS_DATA_DIR"
}
trap cleanup EXIT

pass() { echo "  PASS: $1"; }
fail() { echo "  FAIL: $1"; exit 1; }

echo "=== Channel Hooks Test ==="
echo "data_dir: $BOTBUS_DATA_DIR"
echo "agent:    $BOTBUS_AGENT"
echo

# Ensure data dir is initialized
$BUS init >/dev/null 2>&1 || true

# --- Add a hook ---
echo "1. Add hook"
OUTPUT=$($BUS hooks add \
    --channel test-hook \
    --if-claim-available "agent://test-dev" \
    --cwd /tmp \
    --cooldown 2s \
    -- echo "hook fired")
echo "$OUTPUT"
HOOK_ID=$(echo "$OUTPUT" | grep '^id:' | awk '{print $2}')
[ -n "$HOOK_ID" ] && pass "hook created: $HOOK_ID" || fail "no hook ID in output"
echo

# --- List hooks ---
echo "2. List hooks"
LIST=$($BUS hooks list)
echo "$LIST" | grep -q "$HOOK_ID" && pass "hook appears in list" || fail "hook not in list"
echo

# --- Dry-run test (no claim held → should fire) ---
echo "3. Test hook (dry-run, no claim held)"
TEST=$($BUS hooks test "$HOOK_ID")
echo "$TEST"
echo "$TEST" | grep -q "would_execute: true" && pass "would execute" || fail "expected would_execute: true"
echo

# --- Send message (hook should fire) ---
echo "4. Send message (hook should fire)"
$BUS send test-hook "trigger message" >/dev/null
AUDIT=$(cat "$BOTBUS_DATA_DIR/hooks_audit.jsonl")
echo "$AUDIT" | tail -1 | grep -q '"condition_result":true' && pass "condition passed" || fail "condition should be true"
echo "$AUDIT" | tail -1 | grep -q '"executed":true' && pass "hook executed" || fail "hook should have executed"
echo

# --- Cooldown test ---
echo "5. Send again immediately (cooldown should block)"
$BUS send test-hook "second trigger" >/dev/null
AUDIT2=$(tail -1 "$BOTBUS_DATA_DIR/hooks_audit.jsonl")
echo "$AUDIT2" | grep -q '"reason":"cooldown active"' && pass "cooldown blocked" || fail "expected cooldown block"
echo

# --- Wait for cooldown to expire ---
echo "6. Wait for cooldown (2s)..."
sleep 3

# --- Claim the resource ---
echo "7. Claim agent://test-dev"
$BUS claim "agent://test-dev" -m "working" >/dev/null
pass "claim created"
echo

# --- Send message (hook should NOT fire — claim held) ---
echo "8. Send message (claim held → hook should not fire)"
$BUS send test-hook "trigger with claim" >/dev/null
AUDIT3=$(tail -1 "$BOTBUS_DATA_DIR/hooks_audit.jsonl")
echo "$AUDIT3" | grep -q '"condition_result":false' && pass "condition failed (claim held)" || fail "expected condition_result: false"
echo "$AUDIT3" | grep -q '"executed":false' && pass "hook not executed" || fail "hook should not have executed"
echo

# --- Release claim ---
echo "9. Release claim"
$BUS release --all >/dev/null
pass "claims released"
echo

# --- Wait for cooldown ---
sleep 3

# --- Send again (should fire now) ---
echo "10. Send message (claim released → hook should fire)"
$BUS send test-hook "trigger after release" >/dev/null
AUDIT4=$(tail -1 "$BOTBUS_DATA_DIR/hooks_audit.jsonl")
echo "$AUDIT4" | grep -q '"condition_result":true' && pass "condition passed" || fail "expected condition_result: true"
echo "$AUDIT4" | grep -q '"executed":true' && pass "hook executed" || fail "hook should have executed"
echo

# --- Dry-run test with claim held ---
echo "11. Test hook with claim held (dry-run)"
$BUS claim "agent://test-dev" -m "blocking" >/dev/null
TEST2=$($BUS hooks test "$HOOK_ID")
echo "$TEST2"
echo "$TEST2" | grep -q "would_execute: false" && pass "would not execute" || fail "expected would_execute: false"
$BUS release --all >/dev/null
echo

# --- Remove hook ---
echo "12. Remove hook"
$BUS hooks remove "$HOOK_ID" >/dev/null
LIST2=$($BUS hooks list)
echo "$LIST2" | grep -q "hooks: \[\]" && pass "hook removed" || fail "hook still in list"
echo

# --- Verify audit log has all entries ---
echo "13. Verify audit log"
AUDIT_COUNT=$(wc -l < "$BOTBUS_DATA_DIR/hooks_audit.jsonl")
[ "$AUDIT_COUNT" -ge 4 ] && pass "audit log has $AUDIT_COUNT entries" || fail "expected >= 4 audit entries, got $AUDIT_COUNT"
echo

echo "=== All tests passed ==="
