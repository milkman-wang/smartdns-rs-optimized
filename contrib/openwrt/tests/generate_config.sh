#!/bin/sh
# shellcheck disable=SC2034,SC3043
set -eu

TEST_ROOT="${TMPDIR:-/tmp}/smartdns-openwrt-test.$$"
(umask 077 && mkdir "$TEST_ROOT")
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
INIT_SCRIPT="$SCRIPT_DIR/../smartdns-rs/files/etc/init.d/smartdns"

mock_value()
{
    if [ "${TEST_NEW_FEATURES:-0}" = 1 ]; then
        case "$1.$2" in
            global.num_workers) echo 1; return ;;
            global.cache_size) echo -1; return ;;
            global.cache_mem_size) echo 16MiB; return ;;
            global.max_query_limit) echo 128; return ;;
            global.serve_expired_prefetch_time) echo 300; return ;;
            global.ipset_name) echo '#4:route4,#6:route6'; return ;;
            global.ipset_timeout) echo yes; return ;;
            global.domain) echo home; return ;;
            global.odhcpd_lease_file) echo /tmp/hosts/odhcpd; return ;;
            global.log_syslog|global.audit_soa) echo yes; return ;;
            global.tls_server|global.ddr) echo 1; return ;;
            global.bind_cert_generate) echo auto; return ;;
            global.bind_cert_san) echo 'resolver.home 192.168.1.1'; return ;;
            global.seconddns_ipset_name) echo '#4:second4'; return ;;
            global.seconddns_no_serve_expired) echo 1; return ;;
            dns0.spki_pin) echo 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA='; return ;;
            client0.ipset_name|domain0.ipset_name|list0.ipset_name) echo '#4:rule4'; return ;;
            client0.no_serve_expired|domain0.no_serve_expired|list0.no_serve_expired) echo 1; return ;;
        esac
    fi
	case "$1.$2" in
		global.enabled) echo 1 ;;
		global.port) echo 6053 ;;
		global.webui_enable) echo 1 ;;
		global.webui_bind) echo 127.0.0.1:6080 ;;
		global.auto_set_dnsmasq) echo 1 ;;
		global.tcp_server) echo 1 ;;
		global.ipv6_server) echo 1 ;;
		global.bind_device) echo 1 ;;
		global.bind_device_name) echo br-lan ;;
		global.speed_check_mode) [ "${TEST_SPEED_MODE+x}" = x ] && printf '%s' "$TEST_SPEED_MODE" ;;
		global.response_mode) [ "${TEST_RESPONSE_MODE+x}" = x ] && printf '%s' "$TEST_RESPONSE_MODE" ;;
		global.dualstack_ip_selection) echo 1 ;;
		global.serve_expired) echo 1 ;;
		global.cache_persist) echo 1 ;;
		global.cache_file) echo "$TEST_ROOT/etc/smartdns/smartdns.cache" ;;
		global.cache_size) echo 4096 ;;
		global.log_level) echo warn ;;
		global.log_file) echo "$TEST_ROOT/log/smartdns.log" ;;
		global.log_size) echo 128K ;;
		global.log_num) echo 2 ;;
		global.resolve_local_hostnames) echo 1 ;;
		global.seconddns_enabled) echo 1 ;;
		global.seconddns_port) echo 6553 ;;
		global.seconddns_tcp_server) echo 1 ;;
		global.seconddns_server_group) echo overseas ;;
		global.seconddns_no_rule_addr) echo 1 ;;
		global.seconddns_force_aaaa_soa) echo 1 ;;
		dns0.enabled) echo 1 ;;
		dns0.type) echo https ;;
		dns0.ip) echo https://dns.example/dns-query ;;
		dns0.server_group) echo domestic-v4 ;;
		dns0.tls_host_verify) echo dns.example ;;
		dns0.set_mark) echo 0xff ;;
		client0.enabled) echo 1 ;;
		client0.server_group) echo domestic-v4 ;;
		client0.dualstack_ip_selection) echo no ;;
		client0.force_aaaa_soa) echo 1 ;;
		client0.nftset_name) echo '#4:inet#fw-4#smartdns-v4' ;;
		domain0.server_group) echo domestic-v4 ;;
		domain0.dualstack_ip_selection) echo no ;;
		domain0.force_aaaa_soa) echo 1 ;;
		domain0.nftset_name) echo '#4:inet#fw-4#smartdns-v4' ;;
		domain0.block_domain_set_file) echo "$TEST_ROOT/domain.list" ;;
		list0.enabled) echo 1 ;;
		list0.domain_list_file) echo "$TEST_ROOT/domain.list" ;;
		list0.server_group) echo overseas ;;
		list0.force_aaaa_soa) echo 1 ;;
		list0.addition_flag) echo '-no-cache' ;;
		ip0.enabled) echo 1 ;;
		ip0.blacklist_ip) echo 1 ;;
		ip0.bogus_nxdomain) echo 1 ;;
		ip0.ip_alias) echo '192.0.2.1 192.0.2.2' ;;
		*) return 1 ;;
	esac
}

config_get()
{
	local destination section option default mock_result
	destination="$1"
	section="$2"
	option="$3"
	default="${4:-}"
	mock_result="$(mock_value "$section" "$option" 2>/dev/null || printf '%s' "$default")"
	eval "$destination=\$mock_result"
}

config_get_bool()
{
	config_get "$@"
}

config_load()
{
	return 0
}

config_foreach()
{
	local callback type
	callback="$1"
	type="$2"
	case "$type" in
		smartdns) "$callback" global ;;
		server) "$callback" dns0 ;;
		client-rule) "$callback" client0 ;;
		domain-rule) "$callback" domain0 ;;
		domain-rule-list) "$callback" list0 ;;
		ip-rule) return 0 ;;
		ip-rule-list) "$callback" ip0 ;;
	esac
}

config_list_foreach()
{
	local section option callback
	section="$1"
	option="$2"
	callback="$3"
	case "$section.$option" in
		client0.client_addr) "$callback" 192.168.1.0/24 ;;
		ip0.ip_addr) "$callback" 203.0.113.0/24 ;;
		global.conf_files) "$callback" "$TEST_ROOT/included.conf" ;;
		global.hosts_files) "$callback" "$TEST_ROOT/hosts" ;;
	esac
}

uci()
{
	[ "${1:-}" = "-q" ] && shift
	[ "${1:-}" = "get" ] || return 1
	case "${2:-}" in
		'dhcp.@dnsmasq[0].leasefile') echo "$TEST_ROOT/dhcp.leases" ;;
		'dhcp.@dnsmasq[0].resolvfile') echo "$TEST_ROOT/resolv.conf" ;;
		network.lan.device) echo br-lan ;;
		*) return 1 ;;
	esac
}

# shellcheck source=/dev/null
. "$INIT_SCRIPT"

CONF_DIR="$TEST_ROOT/etc/smartdns"
RUNTIME_DIR="$TEST_ROOT/run"
RUNTIME_CONF="$RUNTIME_DIR/smartdns.conf"
RUNTIME_TMP="$RUNTIME_CONF.tmp"
LOG_DIR="$TEST_ROOT/log"
DOMAIN_SET_DIR="$CONF_DIR/domain-set"
IP_SET_DIR="$CONF_DIR/ip-set"
DOWNLOAD_DIR="$CONF_DIR/download"
CONF_DOWNLOAD_DIR="$CONF_DIR/conf.d"
ADDRESS_CONF="$CONF_DIR/address.conf"
BLACKLIST_CONF="$CONF_DIR/blacklist-ip.conf"
CUSTOM_CONF="$CONF_DIR/custom.conf"
FORWARDING_LIST="$CONF_DIR/domain-forwarding.list"
BLOCK_LIST="$CONF_DIR/domain-block.list"

mkdir -p "$TEST_ROOT/log"
touch "$TEST_ROOT/dhcp.leases" "$TEST_ROOT/resolv.conf" \
	"$TEST_ROOT/domain.list" "$TEST_ROOT/included.conf" "$TEST_ROOT/hosts"
if ! generate_config; then
	echo "configuration generation failed" >&2
	exit 1
fi

assert_line()
{
	grep -Fqx "$1" "$RUNTIME_TMP" || {
		echo "missing generated line: $1" >&2
		exit 1
	}
}

assert_line 'user nobody'
assert_line 'webui-enable yes'
assert_line 'webui-bind 127.0.0.1:6080'
assert_line 'speed-check-mode ping,tcp:80,tcp:443'
assert_line 'response-mode first-ping'
assert_line 'bind 0.0.0.0:6053@br-lan '
assert_line 'bind [::]:6053@br-lan '
assert_line 'bind 127.0.0.1:6053 '
assert_line 'bind [::1]:6053 '
assert_line 'bind-tcp 0.0.0.0:6053@br-lan '
assert_line 'bind-tcp 127.0.0.1:6053 '
assert_line 'bind 0.0.0.0:6553@br-lan  -group overseas -no-rule-addr -force-aaaa-soa'
assert_line 'server-https https://dns.example/dns-query -tls-host-verify dns.example -group domestic-v4 -set-mark 0xff'
assert_line 'client-rules 192.168.1.0/24'
assert_line 'domain-rules /./ -dualstack-ip-selection no -address #6 -nftset #4:inet#fw-4#smartdns-v4'
assert_line 'domain-rules /domain-set:forwarding-list/ -nameserver domestic-v4 -dualstack-ip-selection no -address #6 -nftset #4:inet#fw-4#smartdns-v4'
assert_line "domain-set -name block-upload -file $TEST_ROOT/domain.list"
assert_line 'address /domain-set:block-upload/#'
assert_line 'address /domain-set:block-list/#'
assert_line "domain-set -name domain-list0 -file $TEST_ROOT/domain.list"
assert_line 'domain-rules /domain-set:domain-list0/ -nameserver overseas -address #6 -no-cache'
assert_line 'blacklist-ip 203.0.113.0/24'
assert_line 'bogus-nxdomain 203.0.113.0/24'
assert_line 'ip-alias 203.0.113.0/24 192.0.2.1,192.0.2.2'
assert_line "conf-file $TEST_ROOT/included.conf"
assert_line "hosts-file $TEST_ROOT/hosts"

TEST_SPEED_MODE=none
TEST_RESPONSE_MODE=fastest-response
generate_config
assert_line 'speed-check-mode none'
assert_line 'response-mode fastest-response'

TEST_SPEED_MODE=''
TEST_RESPONSE_MODE=''
generate_config
assert_line 'speed-check-mode ping,tcp:80,tcp:443'
assert_line 'response-mode first-ping'

echo "OpenWrt generated configuration smoke test: OK"

TEST_NEW_FEATURES=1
generate_config
assert_line 'num-workers 1'
assert_line 'cache-size -1'
assert_line 'cache-mem-size 16MiB'
assert_line 'max-query-limit 128'
assert_line 'serve-expired-prefetch-time 300'
assert_line 'ipset /./#4:route4,#6:route6'
assert_line 'ipset-timeout yes'
assert_line 'domain home'
assert_line 'odhcpd-lease-file /tmp/hosts/odhcpd'
assert_line 'log-syslog yes'
assert_line 'audit-SOA yes'
assert_line 'bind-cert-generate auto'
assert_line 'bind-cert-san resolver.home 192.168.1.1'
assert_line 'bind-tls 0.0.0.0:853@br-lan  -ddr'
assert_line 'bind 0.0.0.0:6553@br-lan  -group overseas -no-rule-addr -force-aaaa-soa -ipset #4:second4 -no-serve-expired'
assert_line 'server-https https://dns.example/dns-query -spki-pin AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= -tls-host-verify dns.example -group domestic-v4 -set-mark 0xff'
assert_line 'domain-rules /./ -dualstack-ip-selection no -address #6 -nftset #4:inet#fw-4#smartdns-v4 -ipset #4:rule4 -no-serve-expired'
assert_line 'domain-rules /domain-set:domain-list0/ -nameserver overseas -address #6 -ipset #4:rule4 -no-serve-expired -no-cache'
echo "OpenWrt new feature configuration test: OK"
