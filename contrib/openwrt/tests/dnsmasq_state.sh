#!/bin/sh
# shellcheck disable=SC2034,SC3043
set -eu

TEST_ROOT="${TMPDIR:-/tmp}/smartdns-dnsmasq-test.$$"
STATE_FILE="$TEST_ROOT/uci-state"
mkdir "$TEST_ROOT"
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM
: >"$STATE_FILE"

state_get()
{
	awk -F '\t' -v key="$1" '$1 == key { print substr($0, length($1) + 2); found=1 } END { exit !found }' "$STATE_FILE"
}

state_set()
{
	local key="$1"
	local value="$2"
	local temporary="$STATE_FILE.new"
	awk -F '\t' -v key="$key" '$1 != key' "$STATE_FILE" >"$temporary"
	printf '%s\t%s\n' "$key" "$value" >>"$temporary"
	mv "$temporary" "$STATE_FILE"
}

state_delete()
{
	local key="$1"
	local temporary="$STATE_FILE.new"
	awk -F '\t' -v key="$key" '$1 != key' "$STATE_FILE" >"$temporary"
	mv "$temporary" "$STATE_FILE"
}

uci()
{
	local command argument key value current item kept
	[ "${1:-}" = "-q" ] && shift
	command="${1:-}"
	argument="${2:-}"
	case "$command" in
		get)
			state_get "$argument"
			;;
		set)
			key="${argument%%=*}"
			value="${argument#*=}"
			state_set "$key" "$value"
			;;
		delete)
			state_delete "$argument"
			;;
		add_list)
			key="${argument%%=*}"
			value="${argument#*=}"
			current="$(state_get "$key" 2>/dev/null || true)"
			[ -n "$current" ] && value="$current $value"
			state_set "$key" "$value"
			;;
		del_list)
			key="${argument%%=*}"
			value="${argument#*=}"
			current="$(state_get "$key" 2>/dev/null || true)"
			kept=""
			for item in $current; do
				[ "$item" = "$value" ] || kept="${kept:+$kept }$item"
			done
			if [ -n "$kept" ]; then
				state_set "$key" "$kept"
			else
				state_delete "$key"
			fi
			;;
		commit)
			return 0
			;;
		*)
			return 1
			;;
	esac
}

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
# shellcheck source=/dev/null
. "$SCRIPT_DIR/../smartdns-rs/files/etc/init.d/smartdns"

assert_value()
{
	local expected="$2"
	local actual
	actual="$(state_get "$1" 2>/dev/null || true)"
	if [ "$actual" != "$expected" ]; then
		echo "unexpected UCI value for $1: '$actual' (expected '$expected')" >&2
		exit 1
	fi
}

state_set 'dhcp.@dnsmasq[0].server' '1.1.1.1 9.9.9.9'
state_set 'dhcp.@dnsmasq[0].rebind_protection' '1'
state_set 'dhcp.@dnsmasq[0].domainneeded' '1'

DNSMASQ_CHANGED=0
apply_dnsmasq_forward 6053
assert_value 'dhcp.@dnsmasq[0].server' '127.0.0.1#6053'
assert_value 'dhcp.@dnsmasq[0].noresolv' '1'
assert_value 'dhcp.@dnsmasq[0].rebind_protection' '0'
assert_value 'dhcp.@dnsmasq[0].domainneeded' '0'
assert_value 'smartdns.@smartdns[0].dnsmasq_old_server' '1.1.1.1 9.9.9.9'

restore_dnsmasq
assert_value 'dhcp.@dnsmasq[0].server' '1.1.1.1 9.9.9.9'
assert_value 'dhcp.@dnsmasq[0].noresolv' ''
assert_value 'dhcp.@dnsmasq[0].rebind_protection' '1'
assert_value 'dhcp.@dnsmasq[0].domainneeded' '1'
assert_value 'smartdns.@smartdns[0].dnsmasq_managed' ''

echo "OpenWrt dnsmasq state round-trip test: OK"

signals=""
signal_status=0
procd_send_signal()
{
	signals="${signals}${signals:+ }$1/$2/$3"
	return "$signal_status"
}

apply_dnsmasq_forward 6053
flushcache
[ "$signals" = "smartdns/smartdns/USR1 dnsmasq/*/HUP" ]

signals=""
restore_dnsmasq
flushcache
[ "$signals" = "smartdns/smartdns/USR1" ]

signal_status=252
if output="$(flushcache)"; then
	echo "flushcache succeeded without a running service" >&2
	exit 1
fi
[ "$output" = "SmartDNS-rs is not running." ]

echo "OpenWrt cache flush signals SmartDNS-rs and a forwarding dnsmasq: OK"
