// Construct the view through the public form API to catch missing tabs,
// misplaced sections and fields that disappear from the row editors.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

class Section {
    constructor(kind, name) {
        this.kind = kind;
        this.name = name;
        this.options = {};
        this.tabs = {};
    }
    tab(name) {
        assert.ok(!this.tabs[name], `Duplicate tab: ${name}`);
        this.tabs[name] = true;
    }
    option(kind, name, title, childName) {
        assert.ok(!this.options[name], `Duplicate field: ${this.name}.${name}`);
        const option = {
            kind, name, enabled: '1', disabled: '0',
            value() {}, depends() {}
        };
        if (kind === 'SectionValue')
            option.subsection = new Section(title, childName);
        this.options[name] = option;
        return option;
    }
    taboption(tab, ...args) {
        assert.ok(this.tabs[tab], `Missing tab: ${this.name}.${tab}`);
        const option = this.option(...args);
        option.tab = tab;
        return option;
    }
}
class FormMap {
    constructor() { this.sections = []; }
    section(kind, name) {
        const section = new Section(kind, name);
        this.sections.push(section);
        return section;
    }
    render() { return Promise.resolve(this); }
    prepend(node) { this.stylesheet = node; }
}
const form = { Map: FormMap };
for (const kind of ['NamedSection', 'TypedSection', 'GridSection', 'SectionValue',
    'Flag', 'Value', 'ListValue', 'DynamicList', 'FileUpload', 'TextValue', 'Button'])
    form[kind] = kind;

const source = fs.readFileSync(path.join(__dirname,
    '../luci-app-smartdns-rs/htdocs/luci-static/resources/view/smartdns/smartdns.js'), 'utf8');
let checkResult;
let notification;
const page = new Function('view', 'rpc', 'form', 'uci', '_', 'E', 'L', 'fs', 'ui', source)(
    { extend: value => value }, { declare: () => () => {} }, form,
    { sections: () => {}, get: () => undefined }, value => value,
    (tag, attrs, children) => ({tag, attrs, children}), {resource: path => '/luci-static/resources/' + path},
    {exec: async (command, args) => {
        assert.equal(command, '/etc/init.d/smartdns');
        assert.deepEqual(args, ['check']);
        return checkResult;
    }},
    {addNotification: (title, node, type) => { notification = {node, type}; }});
(async () => {
const map = await page.render();
assert.equal(map.stylesheet.attrs.href, '/luci-static/resources/smartdns/form.css?v=25.7');
const section = name => map.sections.find(s => s.name === name);
const settings = section('smartdns');
assert.deepEqual(Object.keys(settings.tabs),
    ['general', 'advanced', 'second', 'dns64', 'files', 'proxy', 'custom']);
assert.deepEqual(Object.values(settings.options).filter(o => o.tab === 'general').map(o => o.name),
    ['enabled', 'server_name', 'port', 'auto_set_dnsmasq']);
assert.equal(settings.options.proxy_server.tab, 'proxy');
assert.equal(settings.options.dns64.tab, 'dns64');
assert.equal(settings.options.custom_conf.tab, 'custom');
assert.equal(settings.options._downloads.subsection.name, 'download-file');
for (const result of [
    {code: 0, stdout: '', stderr: ''},
    {code: 0, stdout: 'valid with warnings', stderr: ''},
    {code: 1, stdout: '', stderr: 'invalid configuration at line 3'},
    {code: 1, stdout: '', stderr: ''}
]) {
    checkResult = result;
    await settings.options._check.onclick();
    assert.equal(notification.type, result.code === 0 ? 'info' : 'error');
    assert.equal(notification.node.children[0].children[0], result.code === 0
        ? 'Configuration is valid.' : 'Configuration validation failed. Please check the system log.');
    if (result.stderr || result.stdout)
        assert.equal(notification.node.children[1].children[0], result.stderr || result.stdout);
}

const upstream = section('server');
assert.equal(upstream.options.ip.tab, 'general');
for (const name of ['set_mark', 'spki_pin', 'host_name', 'no_check_certificate', 'addition_arg']) {
    assert.equal(upstream.options[name].tab, 'advanced');
    assert.equal(upstream.options[name].modalonly, true);
}
assert.equal(section('client-rule').options.block_domain_set_file.tab, 'block');
const domain = section('domain-rule').options._domain_rules;
assert.equal(domain.tab, 'rules');
assert.equal(domain.subsection.name, 'domain-rule-list');
assert.equal(domain.subsection.options.domain_list_file.rmempty, false);
assert.notEqual(domain.subsection.options.block_domain_type.modalonly, true);
assert.equal(domain.subsection.options.addition_flag.tab, 'advanced');
const ip = section('ip-rule').options._ip_rules;
assert.equal(ip.tab, 'rules');
assert.equal(ip.subsection.name, 'ip-rule-list');
assert.equal(ip.subsection.options.ip_alias.datatype, 'ipaddr("nomask")');
assert.equal(section('ip-rule').options.blacklist_conf.tab, 'blacklist');
assert.ok(!section('domain-rule-list') && !section('ip-rule-list'),
    'Nested rule lists must not also render as duplicate top-level sections');
console.log('LuCI general settings, modal editors and nested rule lists: OK');
})().catch(err => { console.error(err); process.exitCode = 1; });
