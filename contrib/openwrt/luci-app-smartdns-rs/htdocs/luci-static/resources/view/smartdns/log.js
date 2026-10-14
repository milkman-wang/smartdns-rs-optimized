'use strict';
'require dom';
'require fs';
'require poll';
'require ui';
'require view';

var helper = '/usr/libexec/smartdns-rs-call';

return view.extend({
	render: function() {
		var output = E('pre', {
			'style': 'max-height:650px;overflow:auto;white-space:pre-wrap;word-break:break-word'
		}, [ _('Collecting data ...') ]);

		poll.add(function() {
			return fs.exec(helper, [ 'tail' ]).then(function(res) {
				dom.content(output, [ (res.stdout || '').trim() || _('Log is clean.') ]);
				output.scrollTop = output.scrollHeight;
			}).catch(function(err) {
				dom.content(output, [ _('Unable to read log: %s').format(err) ]);
			});
		});

		return E('div', { 'class': 'cbi-map' }, [
			E('h2', {}, [ _('SmartDNS-rs Log') ]),
			E('div', { 'class': 'cbi-section' }, [
				output,
				E('div', { 'class': 'right' }, [
					E('button', {
						'class': 'btn cbi-button cbi-button-remove',
						'click': ui.createHandlerFn(this, function() {
							return fs.exec(helper, [ 'clear_log' ]).then(function() {
								dom.content(output, [ _('Log is clean.') ]);
							});
						})
					}, [ _('Clear Logs') ]),
					' ',
					E('a', {
						'class': 'btn cbi-button cbi-button-action',
						'href': L.url('admin/services/smartdns')
					}, [ _('Back to SmartDNS-rs') ])
				])
			])
		]);
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
