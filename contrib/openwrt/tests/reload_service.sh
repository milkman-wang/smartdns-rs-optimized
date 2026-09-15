#!/bin/sh
# shellcheck disable=SC2034,SC3043
set -eu

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
# shellcheck source=/dev/null
. "$SCRIPT_DIR/../smartdns-rs/files/etc/init.d/smartdns"

events=""
record() { events="${events}${events:+ }$1"; }
check_and_add_sections() { record sections; }
generate_config() { record generate; return "$generate_status"; }
validate_config() { record validate; return "$validate_status"; }
config_foreach() { record cron; }
sync_dnsmasq() { record dnsmasq; return 0; }
restore_dnsmasq() { echo "reload must not restore dnsmasq on invalid input" >&2; exit 1; }
stop() { echo "reload must not stop the working DNS instance" >&2; exit 1; }
start() { echo "reload must not validate again through rc.common start" >&2; exit 1; }
procd_open_service() { record open; }
procd_open_instance() { record instance; }
procd_set_param() {
	if [ "$1" = file ]; then
		shift
		[ "$*" = "$RUNTIME_CONF $CUSTOM_CONF $ADDRESS_CONF $BLACKLIST_CONF $FORWARDING_LIST $BLOCK_LIST" ] || {
			echo "reload does not watch all editable rule files" >&2
			exit 1
		}
	fi
}
procd_close_instance() { record ready; }
procd_close_service() { record update; }

initscript=/etc/init.d/smartdns
SERVICE_ENABLED=1
generate_status=0
validate_status=1
if reload_service; then
	echo "invalid configuration was accepted" >&2
	exit 1
fi
[ "$events" = "sections generate validate" ]

events=""
validate_status=0
reload_service
[ "$events" = "sections generate validate open cron dnsmasq instance ready update" ]

events=""
generate_status=1
if reload_service; then exit 1; fi
[ "$events" = "sections generate" ]

events=""
generate_status=0
SERVICE_ENABLED=0
reload_service
[ "$events" = "sections generate open cron dnsmasq update" ]
echo "Reload preserves service on invalid config and validates once before updating: OK"
