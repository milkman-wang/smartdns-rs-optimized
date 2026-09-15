'use strict';
'require dom';
'require fs';
'require form';
'require poll';
'require rpc';
'require uci';
'require ui';
'require view';

var callServiceList = rpc.declare({
	object: 'service',
	method: 'list',
	params: [ 'name' ],
	expect: { '': {} }
});

var pollAdded = false;

function getServiceStatus() {
	return L.resolveDefault(callServiceList('smartdns'), {}).then(function(res) {
		return !!(res.smartdns && res.smartdns.instances &&
			res.smartdns.instances.smartdns && res.smartdns.instances.smartdns.running);
	});
}

function renderServiceStatus(running) {
	var enabled = uci.get_first('smartdns', 'smartdns', 'enabled') === '1';
	var port = uci.get_first('smartdns', 'smartdns', 'port') || '6053';
	var autoDnsmasq = uci.get_first('smartdns', 'smartdns', 'auto_set_dnsmasq') === '1';
	var text = running ? _('RUNNING') : _('NOT RUNNING');
	var color = running ? 'green' : 'red';
	var nodes = [ E('span', { 'style': 'color:%s;font-weight:bold'.format(color) },
		[ 'SmartDNS-rs - ' + text ]) ];

	if (!running && enabled)
		nodes.push(E('div', { 'style': 'color:red;font-weight:bold' },
			[ _('Please check the system log and validate the generated configuration.') ]));

	if (running && autoDnsmasq && port !== '53') {
		var servers = uci.get_first('dhcp', 'dnsmasq', 'server') || [];
		if (!Array.isArray(servers))
			servers = [ servers ];
		if (servers.indexOf('127.0.0.1#' + port) < 0)
			nodes.push(E('div', { 'style': 'color:red;font-weight:bold' },
				[ _('Dnsmasq is not forwarding to SmartDNS-rs.') ]));
	}

	return nodes;
}

function addSpeedModes(o, withDefault) {
	if (withDefault)
		o.value('', _('Default'));
	o.value('ping,tcp:80,tcp:443');
	o.value('ping,tcp:443,tcp:80');
	o.value('tcp:80,tcp:443,ping');
	o.value('tcp:443,tcp:80,ping');
	o.value('http:80,https:443,ping');
	o.value('none', _('None'));
	o.validate = function(sectionId, value) {
		if (!value || value === 'none')
			return true;
		var modes = value.split(',');
		for (var i = 0; i < modes.length; i++) {
			if (modes[i] === 'ping' || modes[i] === 'http' || modes[i] === 'https')
				continue;
			if (/^(tcp|tcp-syn|http|https):[0-9]+$/.test(modes[i]))
				continue;
			return _('Supported modes are ping, tcp:PORT, tcp-syn:PORT, http[:PORT], https[:PORT], and none.');
		}
		return true;
	};
}

function validateNftset(sectionId, value) {
	if (!value)
		return true;
	var sets = value.split(',');
	for (var i = 0; i < sets.length; i++) {
		if (!/^#[46]:(inet|ip|ip6)#[A-Za-z0-9_-]+#[A-Za-z0-9_-]+$/.test(sets[i]))
			return _('NFT set format: #4:family#table#set,#6:family#table#set');
	}
	return true;
}

function validateCacheFile(sectionId, value) {
	if (!value || /^\/etc\/smartdns\/[^/]+\.cache$/.test(value))
		return true;
	return _('Cache file must be /etc/smartdns/NAME.cache.');
}

function validateLogFile(sectionId, value) {
	if (/^\/var\/log\/smartdns\/[^/]+$/.test(value))
		return true;
	return _('Log files must be directly under /var/log/smartdns/.');
}

function validateDownloadName(sectionId, value) {
	if (/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(value))
		return true;
	return _('File names may contain only letters, numbers, dots, underscores, and hyphens, and must not start with a dot.');
}

function validatePacketMark(sectionId, value) {
	if (!value)
		return true;
	if (!/^(?:0|[1-9][0-9]*|0[xX][0-9a-fA-F]+)$/.test(value))
		return _('Packet mark must be a 32-bit decimal or hexadecimal number.');
	var mark = Number(value);
	if (!isFinite(mark) || mark < 0 || mark > 0xffffffff)
		return _('Packet mark must be a 32-bit decimal or hexadecimal number.');
	return true;
}

function textFileOption(section, tab, name, title, description, path, rows) {
	var o = tab != null
		? section.taboption(tab, form.TextValue, name, title, description)
		: section.option(form.TextValue, name, title, description);
	o.rows = rows || 12;
	o.monospace = true;
	// Soft wrapping keeps long rules selectable without horizontal dragging.
	// It does not insert line breaks into the saved configuration.
	o.wrap = true;
	o.cfgvalue = function() {
		return L.resolveDefault(fs.trimmed(path), '');
	};
	o.write = function(sectionId, value) {
		value = (value || '').trim().replace(/\r\n/g, '\n');
		return fs.write(path, value ? value + '\n' : '');
	};
	o.remove = function(sectionId) {
		return this.write(sectionId, '');
	};
	return o;
}

return view.extend({
	handleSaveApply: function(ev, mode) {
		return this.handleSave(ev).then(function() {
			return uci.changes();
		}).then(function(changes) {
			// UCI changes trigger the service reload through procd. File-only
			// edits need an explicit reload, including Save followed by Apply.
			if (changes.smartdns && changes.smartdns.length)
				return ui.changes.apply(mode == '0');

			return fs.exec('/etc/init.d/smartdns', [ 'reload' ]).then(function(result) {
				if (result.code !== 0)
					throw new Error(result.stderr || result.stdout || _('Configuration validation failed. Please check the system log.'));
				if (Object.keys(changes).length)
					return ui.changes.apply(mode == '0');
				ui.addNotification(null, E('p', {}, [ _('Configuration changes applied.') ]), 'info');
			});
		}).catch(function(err) {
			ui.addNotification(null, E('p', {}, [ err.message ]), 'error');
			throw err;
		});
	},

	load: function() {
		return Promise.all([
			uci.load('smartdns'),
			uci.load('dhcp'),
			uci.load('network')
		]);
	},

	render: function() {
		var m, s, o, ss, so;
		var groups = [];
		uci.sections('smartdns', 'server', function(server) {
			if (server.server_group && groups.indexOf(server.server_group) < 0)
				groups.push(server.server_group);
		});

		m = new form.Map('smartdns', _('SmartDNS-rs'));
		m.description = _('Rust implementation of SmartDNS with OpenWrt procd, UCI, dnsmasq and nftables integration.');

		s = m.section(form.NamedSection, '_status');
		s.render = function() {
			var box = E('div', { 'class': 'cbi-section', 'id': 'service_status' },
				[ _('Collecting data ...') ]);
			var refresh = function() {
				return getServiceStatus().then(function(running) {
					dom.content(box, renderServiceStatus(running));
				});
			};
			if (!pollAdded) {
				poll.add(refresh, 2);
				pollAdded = true;
			}
			refresh();
			return box;
		};

		s = m.section(form.TypedSection, 'smartdns', _('Settings'));
		s.anonymous = true;
		s.addremove = false;
		s.tab('general', _('General Settings'));
		s.tab('advanced', _('Advanced Settings'));
		s.tab('listeners', _('Encrypted Listeners'));
		s.tab('second', _('Second Server'));
		s.tab('files', _('Files and Updates'));
		s.tab('logging', _('Logging'));
		s.tab('custom', _('Custom Settings'));

		o = s.taboption('general', form.Flag, 'enabled', _('Enable'));
		o.rmempty = false;
		o.default = o.disabled;

		o = s.taboption('general', form.Value, 'server_name', _('Server Name'));
		o.datatype = 'hostname';
		o.rmempty = true;

		o = s.taboption('general', form.Value, 'port', _('Local Port'),
			_('Port 6053 is recommended with dnsmasq forwarding. Port 53 makes SmartDNS-rs the main DNS listener.'));
		o.datatype = 'port';
		o.default = '6053';
		o.rmempty = false;

		o = s.taboption('general', form.Flag, 'auto_set_dnsmasq', _('Automatically Set Dnsmasq'),
			_('Preserves existing dnsmasq settings and restores them when SmartDNS-rs stops.'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('general', form.Flag, 'tcp_server', _('TCP Server'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('general', form.Flag, 'ipv6_server', _('IPv6 Server'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('general', form.Flag, 'bind_device', _('Bind Device'),
			_('Listen on the selected interface and keep loopback listeners for router-local DNS queries.'));
		o.default = o.disabled;
		o.rmempty = false;

		o = s.taboption('general', form.Value, 'bind_device_name', _('Bind Device Name'));
		o.placeholder = uci.get('network', 'lan', 'device') || 'br-lan';
		o.depends('bind_device', '1');

		o = s.taboption('general', form.Value, 'speed_check_mode', _('Speed Check Mode'),
			_('Default: ping,tcp:80,tcp:443. Tests address reachability in this order. Use none to disable response address speed checks.'));
		addSpeedModes(o, false);
		o.default = 'ping,tcp:80,tcp:443';
		o.rmempty = false;

		o = s.taboption('general', form.ListValue, 'response_mode', _('Response Mode'),
			_('First Ping returns after the first successful speed check. Fastest IP waits to compare addresses. Fastest Response returns the upstream answer without response address speed checks.'));
		o.default = 'first-ping';
		o.rmempty = false;
		o.value('first-ping', _('First Ping'));
		o.value('fastest-ip', _('Fastest IP'));
		o.value('fastest-response', _('Fastest Response'));

		o = s.taboption('general', form.Flag, 'dualstack_ip_selection', _('Dual-stack IP Selection'),
			_('Compares IPv4 and IPv6 reachability separately and may add DNS latency. Disable to return both families without this comparison.'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('advanced', form.Value, 'num_workers', _('Worker Threads'));
		o.datatype = 'range(1,128)';
		o.placeholder = "2";

		o = s.taboption('advanced', form.Value, 'cache_mem_size', _('Cache Memory Budget'));
		o.placeholder = "16MiB";

		o = s.taboption('advanced', form.Value, 'max_query_limit', _('Maximum Concurrent Queries'));
		o.datatype = 'uinteger';
		o.placeholder = "0";

		o = s.taboption('advanced', form.Value, 'serve_expired_ttl', _('Maximum Stale Lifetime'));
		o.datatype = 'uinteger';

		o = s.taboption('advanced', form.Value, 'serve_expired_reply_ttl', _('Stale Reply TTL'));
		o.datatype = 'uinteger';

		o = s.taboption('advanced', form.Value, 'serve_expired_prefetch_time', _('Expired Cache Refresh Interval'));
		o.datatype = 'uinteger';
		o.placeholder = "300";

		o = s.taboption('advanced', form.Value, 'domain', _('Local Domain Suffix'));
		o.placeholder = "home";

		o = s.taboption('advanced', form.Value, 'odhcpd_lease_file', _('Odhcpd Lease File'));
		o.placeholder = "/tmp/hosts/odhcpd";

		o = s.taboption('advanced', form.ListValue, 'webui_enable', _('Enable Built-in WebUI (WebUI Package Required)'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('advanced', form.Value, 'webui_bind', _('Built-in WebUI Listen Address'));
		o.placeholder = "127.0.0.1:6080";

		o = s.taboption('advanced', form.Value, 'ipset_name', _('Kernel IP Set'));
		o.placeholder = "#4:route4,#6:route6";

		o = s.taboption('advanced', form.Value, 'nftset_name', _('NFT Set'));
		o.placeholder = "#4:inet#fw4#route4";

		o = s.taboption('advanced', form.ListValue, 'ipset_timeout', _('IP Set Entry Timeouts'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('advanced', form.ListValue, 'nftset_timeout', _('NFT Set Entry Timeouts'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('advanced', form.Value, 'ipset_no_speed', _('IP Sets for Failed Probes'));
		o.placeholder = "#4:slow4,#6:slow6";

		o = s.taboption('advanced', form.Value, 'nftset_no_speed', _('NFT Sets for Failed Probes'));
		o.placeholder = "#4:inet#fw4#slow4";

		o = s.taboption('advanced', form.ListValue, 'nftset_debug', _('NFT Set Debug Logging'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('advanced', form.Flag, 'prefetch_domain', _('Domain Prefetch'));
		o.default = o.disabled;

		o = s.taboption('advanced', form.Flag, 'serve_expired', _('Serve Expired'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('advanced', form.Value, 'cache_size', _('Cache Size'));
		o.datatype = 'integer';
		o.validate = function(section, value) { return /^(-1|[0-9]+)$/.test(value) || _('Cache size must be -1, 0, or a positive integer.'); };
		o.default = '4096';

		o = s.taboption('advanced', form.Flag, 'cache_persist', _('Cache Persist'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('advanced', form.Value, 'cache_file', _('Cache File'));
		o.placeholder = '/etc/smartdns/smartdns.cache';
		o.validate = validateCacheFile;
		o.depends('cache_persist', '1');

		o = s.taboption('advanced', form.Flag, 'resolve_local_hostnames', _('Resolve Local Hostnames'));
		o.default = o.enabled;
		o.rmempty = false;

		o = s.taboption('advanced', form.Flag, 'mdns_lookup', _('mDNS Lookup'));
		o.default = o.disabled;

		o = s.taboption('advanced', form.Flag, 'force_aaaa_soa', _('Force AAAA SOA'));
		o.default = o.disabled;

		o = s.taboption('advanced', form.Flag, 'force_https_soa', _('Force HTTPS SOA'));
		o.default = o.enabled;
		o.rmempty = false;

	[ [ 'rr_ttl', _('Domain TTL') ],
	  [ 'rr_ttl_min', _('Minimum Domain TTL') ],
	  [ 'rr_ttl_max', _('Maximum Domain TTL') ],
	  [ 'rr_ttl_reply_max', _('Maximum Reply TTL') ] ].forEach(function(item) {
		o = s.taboption('advanced', form.Value, item[0], item[1]);
		o.datatype = 'uinteger';
	});

		o = s.taboption('advanced', form.Value, 'server_flags', _('Additional Listener Arguments'));
		o.rmempty = true;

		o = s.taboption('advanced', form.Value, 'dns64', _('DNS64 Prefix'));
		o.datatype = 'cidr6';
		o.placeholder = '64:ff9b::/96';

		o = s.taboption('listeners', form.Flag, 'ddr', _('Advertise Encrypted Listeners (DDR)'));

		o = s.taboption('listeners', form.ListValue, 'bind_cert_generate', _('Automatic Local Certificate Generation'));
		o.value('', _('Default'));
		o.value('auto', _('Automatic'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('listeners', form.DynamicList, 'bind_cert_san', _('Certificate Names and IP Addresses'));
		o.placeholder = "resolver.home";

		o = s.taboption('listeners', form.Value, 'bind_cert_validity_days', _('Certificate Validity (Days)'));
		o.datatype = 'uinteger';
		o.placeholder = "390";

		o = s.taboption('listeners', form.Value, 'bind_cert_root_key_file', _('Local CA Key File'));
		o.placeholder = "/etc/smartdns/smartdns-root-key.pem";

		o = s.taboption('listeners', form.Flag, 'tls_server', _('DNS-over-TLS Server'));
		o.default = o.disabled;
		o = s.taboption('listeners', form.Value, 'tls_server_port', _('DNS-over-TLS Port'));
		o.datatype = 'port';
		o.default = '853';
		o.depends('tls_server', '1');

		o = s.taboption('listeners', form.Flag, 'doh_server', _('DNS-over-HTTPS Server'));
		o.default = o.disabled;
		o = s.taboption('listeners', form.Value, 'doh_server_port', _('DNS-over-HTTPS Port'));
		o.datatype = 'port';
		o.default = '8443';
		o.depends('doh_server', '1');

		o = s.taboption('listeners', form.Value, 'bind_cert', _('Server Certificate'));
		o.placeholder = '/etc/smartdns/server.pem';
		o.depends('tls_server', '1');
		o.depends('doh_server', '1');
		o = s.taboption('listeners', form.Value, 'bind_cert_key', _('Server Certificate Key'));
		o.placeholder = '/etc/smartdns/server-key.pem';
		o.depends('tls_server', '1');
		o.depends('doh_server', '1');
		o = s.taboption('listeners', form.Value, 'bind_cert_key_pass', _('Certificate Key Password'));
		o.password = true;
		o.depends('tls_server', '1');
		o.depends('doh_server', '1');

		o = s.taboption('second', form.Flag, 'seconddns_enabled', _('Enable Second Server'));
		o.default = o.disabled;
		o = s.taboption('second', form.Value, 'seconddns_port', _('Second Server Port'));
		o.datatype = 'port';
		o.default = '6553';
		o.depends('seconddns_enabled', '1');
		o = s.taboption('second', form.Flag, 'seconddns_tcp_server', _('TCP Server'));
		o.default = o.enabled;
		o.depends('seconddns_enabled', '1');
		o = s.taboption('second', form.Value, 'seconddns_server_group', _('Upstream Server Group'));
		groups.forEach(function(group) { o.value(group); });
		o.depends('seconddns_enabled', '1');
	[ [ 'no_speed_check', _('Skip Speed Check') ],
	  [ 'no_rule_addr', _('Skip Address Rules') ],
	  [ 'no_rule_nameserver', _('Skip Nameserver Rules') ],
	  [ 'no_rule_ipset', _('Skip IP Set and NFT Set Rules') ],
	  [ 'no_rule_soa', _('Skip SOA Address Rules') ],
	  [ 'no_dualstack_selection', _('Skip Dual-stack Selection') ],
	  [ 'no_cache', _('Skip Cache') ],
	  [ 'force_aaaa_soa', _('Force AAAA SOA') ],
	  [ 'force_https_soa', _('Force HTTPS SOA') ] ].forEach(function(item) {
		o = s.taboption('second', form.Flag, 'seconddns_' + item[0], item[1]);
		o.depends('seconddns_enabled', '1');
	});
		o = s.taboption('second', form.Value, 'seconddns_ipset_name', _('Kernel IP Set'));
		o.placeholder = "#4:route4,#6:route6";
		o.depends('seconddns_enabled', '1');

		o = s.taboption('second', form.Value, 'seconddns_nftset_name', _('NFT Set'));
		o.placeholder = "#4:inet#fw4#route4";
		o.depends('seconddns_enabled', '1');

		o = s.taboption('second', form.Flag, 'seconddns_no_serve_expired', _('Disable Stale Replies'));
		o.depends('seconddns_enabled', '1');

		o = s.taboption('second', form.Value, 'seconddns_server_flags', _('Additional Listener Arguments'));
		o.depends('seconddns_enabled', '1');

		o = s.taboption('files', form.Flag, 'enable_auto_update', _('Enable Auto Update'));
		o.default = o.disabled;
		o = s.taboption('files', form.ListValue, 'auto_update_week_time', _('Update Day'));
		o.value('*', _('Every Day'));
		o.value('0', _('Sunday'));
		o.value('1', _('Monday'));
		o.value('2', _('Tuesday'));
		o.value('3', _('Wednesday'));
		o.value('4', _('Thursday'));
		o.value('5', _('Friday'));
		o.value('6', _('Saturday'));
		o.depends('enable_auto_update', '1');
		o = s.taboption('files', form.Value, 'auto_update_day_time', _('Update Hour'));
		o.datatype = 'range(0,23)';
		o.default = '5';
		o.depends('enable_auto_update', '1');

		o = s.taboption('files', form.DynamicList, 'conf_files', _('Include Config Files'));
		uci.sections('smartdns', 'download-file', function(file) {
			if (file.type === 'config' && file.name) o.value(file.name);
		});
		o = s.taboption('files', form.DynamicList, 'hosts_files', _('Hosts Files'));
		uci.sections('smartdns', 'download-file', function(file) {
			if (file.type === 'hosts' && file.name) o.value(file.name);
		});

		o = s.taboption('files', form.FileUpload, 'upload_conf_file', _('Upload Config File'));
		o.root_directory = '/etc/smartdns/conf.d';
		o.cfgvalue = function() { return ''; };
		o.write = function() {};
		o.remove = function() {};
		o = s.taboption('files', form.FileUpload, 'upload_list_file', _('Upload Domain List File'));
		o.root_directory = '/etc/smartdns/domain-set';
		o.cfgvalue = function() { return ''; };
		o.write = function() {};
		o.remove = function() {};
		o = s.taboption('files', form.FileUpload, 'upload_other_file', _('Upload File'));
		o.root_directory = '/etc/smartdns';
		o.cfgvalue = function() { return ''; };
		o.write = function() {};
		o.remove = function() {};

		o = s.taboption('files', form.SectionValue, '_downloads', form.GridSection,
			'download-file', _('Download Files'));
		ss = o.subsection;
		ss.anonymous = true;
		ss.addremove = true;
		ss.sortable = true;
		so = ss.option(form.Value, 'name', _('File Name'));
		so.validate = validateDownloadName;
		so.rmempty = false;
		so = ss.option(form.Value, 'url', _('URL'));
		so.datatype = 'url';
		so.rmempty = false;
		so = ss.option(form.ListValue, 'type', _('Type'));
		so.value('list', _('Domain List'));
		so.value('config', _('Configuration'));
		so.value('ip-set', _('IP Set'));
		so.value('hosts', _('Hosts'));
		so.rmempty = false;
		so = ss.option(form.Value, 'desc', _('Description'));
		so = ss.option(form.Flag, 'use_proxy', _('Use Proxy'));

		o = s.taboption('files', form.Button, '_update', _('Update Files Now'));
		o.inputtitle = _('Update');
		o.inputstyle = 'apply';
		o.onclick = function() {
			return fs.exec('/etc/init.d/smartdns', [ 'updatefiles' ]).then(function(res) {
				ui.addNotification(null, E('p', {}, [ res.stdout || _('Files updated.') ]));
			});
		};

		o = s.taboption('logging', form.ListValue, 'log_syslog', _('Send Logs to Syslog'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('logging', form.ListValue, 'audit_soa', _('Include SOA in Audit Logs'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('logging', form.ListValue, 'audit_console', _('Audit to Console'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('logging', form.ListValue, 'audit_syslog', _('Audit to Syslog'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('logging', form.ListValue, 'debug_save_fail_packet', _('Capture Malformed DNS Packets'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));

		o = s.taboption('logging', form.Value, 'debug_save_fail_packet_dir', _('Malformed Packet Directory'));
		o.placeholder = "/tmp/smartdns";

		o = s.taboption('logging', form.ListValue, 'log_level', _('Log Level'));
		o.value('error', _('Error'));
		o.value('warn', _('Warning'));
		o.value('info', _('Information'));
		o.value('debug', _('Debug'));
		o.default = 'warn';
		o = s.taboption('logging', form.Value, 'log_file', _('Log File'));
		o.default = '/var/log/smartdns/smartdns.log';
		o.validate = validateLogFile;
		o = s.taboption('logging', form.Value, 'log_size', _('Log Size'));
		o.default = '128K';
		o = s.taboption('logging', form.Value, 'log_num', _('Log Number'));
		o.datatype = 'uinteger';
		o.default = '2';
		o = s.taboption('logging', form.Flag, 'enable_audit_log', _('Enable Audit Log'));
		o.default = o.disabled;
		o = s.taboption('logging', form.Value, 'audit_log_file', _('Audit Log File'));
		o.default = '/var/log/smartdns/smartdns-audit.log';
		o.validate = validateLogFile;
		o.depends('enable_audit_log', '1');
		o = s.taboption('logging', form.Value, 'audit_log_size', _('Audit Log Size'));
		o.default = '128K';
		o.depends('enable_audit_log', '1');
		o = s.taboption('logging', form.Value, 'audit_log_num', _('Audit Log Number'));
		o.datatype = 'uinteger';
		o.default = '2';
		o.depends('enable_audit_log', '1');
		o = s.taboption('logging', form.Button, '_view_log', _('View Log'));
		o.inputtitle = _('Open Log Page');
		o.inputstyle = 'action';
		o.onclick = function() { window.location.href = L.url('admin/services/smartdns/log'); };

		o = s.taboption('custom', form.Value, 'proxy_server', _('Proxy Server URL'));
		o.placeholder = 'socks5://127.0.0.1:1080';
		o = s.taboption('custom', form.Flag, 'coredump', _('Enable Coredump'),
			_('Allow core dumps through procd. The actual file location follows the kernel core pattern.'));
		o.default = o.disabled;
		textFileOption(s, 'custom', 'custom_conf', _('Native Configuration'),
			_('SmartDNS-rs directives in this file are included after generated settings.'),
			'/etc/smartdns/custom.conf', 18);

		o = s.taboption('custom', form.Button, '_check', _('Validate Configuration'));
		o.inputtitle = _('Validate');
		o.inputstyle = 'apply';
		o.onclick = function() {
			return fs.exec('/etc/init.d/smartdns', [ 'check' ]).then(function(res) {
				ui.addNotification(null, E('p', {}, [ res.stdout || _('Configuration is valid.') ]));
			});
		};

		s = m.section(form.GridSection, 'server', _('Upstream DNS Servers'));
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;
		o = s.option(form.Flag, 'enabled', _('Enable'));
		o.default = o.enabled;
		o.editable = true;
		o = s.option(form.Value, 'name', _('Name'));
		o = s.option(form.Value, 'ip', _('Address or URL'),
			_('Enter an IP address, hostname, or a complete DNS URL. The separate port is ignored for complete URLs.'));
		o.rmempty = false;
		o = s.option(form.Value, 'port', _('Port'));
		o.datatype = 'port';
		o.modalonly = true;
		o = s.option(form.ListValue, 'type', _('Protocol'));
		o.value('udp', _('UDP'));
		o.value('tcp', _('TCP'));
		o.value('tls', _('DNS over TLS'));
		o.value('https', _('DNS over HTTPS'));
		o.value('quic', _('DNS over QUIC'));
		o.value('h3', _('DNS over HTTP/3'));
		o.default = 'udp';
		o.rmempty = false;
		o = s.option(form.Value, 'server_group', _('Server Group'));
		o = s.option(form.Flag, 'exclude_default_group', _('Exclude Default Group'));
		o.modalonly = true;
		o = s.option(form.Flag, 'blacklist_ip', _('Blacklist IP Filtering'));
		o.modalonly = true;
		o = s.option(form.Flag, 'check_edns', _('Require EDNS'));
		o.modalonly = true;
		o = s.option(form.Value, 'spki_pin', _('TLS SPKI Pin (Base64 SHA-256)'));
		o.modalonly = true;

		o = s.option(form.Value, 'tls_host_verify', _('TLS Hostname Verify'));
		o.modalonly = true;
		o = s.option(form.Value, 'host_name', _('TLS SNI Name'));
		o.modalonly = true;
		o = s.option(form.Flag, 'no_check_certificate', _('Disable Certificate Verification'));
		o.modalonly = true;
		o = s.option(form.Value, 'set_mark', _('Packet Mark'));
		o.validate = validatePacketMark;
		o.modalonly = true;
		o = s.option(form.Flag, 'use_proxy', _('Use Proxy'));
		o.modalonly = true;
		o = s.option(form.Value, 'addition_arg', _('Additional Server Arguments'));
		o.modalonly = true;

		s = m.section(form.GridSection, 'client-rule', _('Client Rules'));
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;
		o = s.option(form.Flag, 'enabled', _('Enable'));
		o.default = o.enabled;
		o.editable = true;
		o = s.option(form.DynamicList, 'client_addr', _('Client Address'),
			_('IPv4/IPv6 subnet or MAC address.'));
		o.rmempty = false;
		o = s.option(form.Value, 'server_group', _('Server Group'));
		groups.forEach(function(group) { o.value(group); });
		o = s.option(form.ListValue, 'speed_check_mode', _('Speed Check Mode'));
		addSpeedModes(o, true);
		o.modalonly = true;
		o = s.option(form.ListValue, 'dualstack_ip_selection', _('Dual-stack Selection'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));
		o.modalonly = true;
		o = s.option(form.Flag, 'force_aaaa_soa', _('Force AAAA SOA'));
		o.modalonly = true;
		o = s.option(form.Value, 'ipset_name', _('Kernel IP Set'));
		o.placeholder = "#4:route4,#6:route6";
		o.modalonly = true;

		o = s.option(form.Flag, 'no_serve_expired', _('Disable Stale Replies'));
		o.modalonly = true;

		o = s.option(form.Value, 'nftset_name', _('NFT Set'));
		o.validate = validateNftset;
		o.modalonly = true;
		o = s.option(form.FileUpload, 'block_domain_set_file', _('Block Domain File'));
		o.root_directory = '/etc/smartdns/domain-set';
		o.modalonly = true;

		s = m.section(form.TypedSection, 'domain-rule', _('Domain Rules'));
		s.anonymous = true;
		s.addremove = false;
		s.tab('forward', _('Forwarding'));
		s.tab('block', _('Blocking'));
		s.tab('address', _('Static Addresses'));
		o = s.taboption('forward', form.Value, 'server_group', _('Server Group'));
		groups.forEach(function(group) { o.value(group); });
		o = s.taboption('forward', form.ListValue, 'speed_check_mode', _('Speed Check Mode'));
		addSpeedModes(o, true);
		o = s.taboption('forward', form.ListValue, 'dualstack_ip_selection', _('Dual-stack Selection'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));
		o = s.taboption('forward', form.Flag, 'force_aaaa_soa', _('Force AAAA SOA'));
		o = s.taboption('forward', form.Value, 'ipset_name', _('Kernel IP Set'));
		o.placeholder = "#4:route4,#6:route6";

		o = s.taboption('forward', form.Flag, 'no_serve_expired', _('Disable Stale Replies'));

		o = s.taboption('forward', form.Value, 'nftset_name', _('NFT Set'));
		o.validate = validateNftset;
		o = s.taboption('forward', form.FileUpload, 'forwarding_domain_set_file', _('Forwarding Domain File'));
		o.root_directory = '/etc/smartdns/domain-set';
		o = s.taboption('forward', form.Value, 'addition_flag', _('Additional Rule Arguments'));
		textFileOption(s, 'forward', 'domain_forwarding_list', _('Forwarding Domain List'),
			_('One domain per line.'), '/etc/smartdns/domain-forwarding.list', 12);
		o = s.taboption('block', form.FileUpload, 'block_domain_set_file', _('Block Domain File'));
		o.root_directory = '/etc/smartdns/domain-set';
		textFileOption(s, 'block', 'domain_block_list', _('Blocked Domain List'),
			_('One domain per line.'), '/etc/smartdns/domain-block.list', 12);
		textFileOption(s, 'address', 'address_conf', _('Address Rules'),
			_('Native address directives, for example: address /example.com/192.0.2.1'),
			'/etc/smartdns/address.conf', 16);

		s = m.section(form.GridSection, 'domain-rule-list', _('Domain Rule Lists'));
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;
		o = s.option(form.Flag, 'enabled', _('Enable'));
		o.default = o.enabled;
		o.editable = true;
		o = s.option(form.Value, 'name', _('Name'));
		o = s.option(form.FileUpload, 'domain_list_file', _('Domain List File'));
		o.root_directory = '/etc/smartdns/domain-set';
		o.rmempty = false;
		o = s.option(form.Value, 'server_group', _('Server Group'));
		groups.forEach(function(group) { o.value(group); });
		o = s.option(form.ListValue, 'block_domain_type', _('Block Type'));
		o.value('', _('None'));
		o.value('all', _('IPv4 and IPv6'));
		o.value('ipv4', _('IPv4'));
		o.value('ipv6', _('IPv6'));
		o.modalonly = true;
		o = s.option(form.ListValue, 'speed_check_mode', _('Speed Check Mode'));
		addSpeedModes(o, true);
		o.modalonly = true;
		o = s.option(form.ListValue, 'dualstack_ip_selection', _('Dual-stack Selection'));
		o.value('', _('Default'));
		o.value('yes', _('Yes'));
		o.value('no', _('No'));
		o.modalonly = true;
		o = s.option(form.Flag, 'force_aaaa_soa', _('Force AAAA SOA'));
		o.modalonly = true;
		o = s.option(form.Value, 'ipset_name', _('Kernel IP Set'));
		o.placeholder = "#4:route4,#6:route6";
		o.modalonly = true;

		o = s.option(form.Flag, 'no_serve_expired', _('Disable Stale Replies'));
		o.modalonly = true;

		o = s.option(form.Value, 'nftset_name', _('NFT Set'));
		o.validate = validateNftset;
		o.modalonly = true;
		o = s.option(form.Value, 'addition_flag', _('Additional Rule Arguments'));
		o.modalonly = true;

		s = m.section(form.GridSection, 'ip-rule-list', _('IP Rules'));
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;
		o = s.option(form.Flag, 'enabled', _('Enable'));
		o.default = o.enabled;
		o.editable = true;
		o = s.option(form.Value, 'name', _('Name'));
		o = s.option(form.DynamicList, 'ip_addr', _('IP Addresses'));
		o.datatype = 'ipaddr';
		o = s.option(form.FileUpload, 'ip_set_file', _('IP Set File'));
		o.root_directory = '/etc/smartdns/ip-set';
		o.modalonly = true;
	[ [ 'whitelist_ip', _('Whitelist IP') ],
	  [ 'blacklist_ip', _('Blacklist IP') ],
	  [ 'ignore_ip', _('Ignore IP') ],
	  [ 'bogus_nxdomain', _('Bogus NXDOMAIN') ] ].forEach(function(item) {
		o = s.option(form.Flag, item[0], item[1]);
		o.modalonly = true;
	});
		o = s.option(form.DynamicList, 'ip_alias', _('IP Alias Targets'));
		o.datatype = 'ipaddr("nomask")';
		o.modalonly = true;

		s = m.section(form.TypedSection, 'ip-rule', _('IP Blacklist'));
		s.anonymous = true;
		s.addremove = false;
		textFileOption(s, null, 'blacklist_conf', _('Blacklist IP Configuration'),
			_('Native blacklist-ip directives.'), '/etc/smartdns/blacklist-ip.conf', 14);

		s = m.section(form.TypedSection, 'smartdns', _('Service Actions'));
		s.anonymous = true;
		s.addremove = false;
		o = s.option(form.Button, '_restart', _('Restart Service'));
		o.inputtitle = _('Restart');
		o.inputstyle = 'apply';
		o.onclick = function() {
			return fs.exec('/etc/init.d/smartdns', [ 'restart' ]).catch(function(err) {
				ui.addNotification(null, E('p', {}, [ err.message ]), 'error');
			});
		};

		return m.render();
	}
});
