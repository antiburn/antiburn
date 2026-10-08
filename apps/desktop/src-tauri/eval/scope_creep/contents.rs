pub fn billing() -> &'static str {
    r#"use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Invoice {
    pub number: u64,
    pub account: String,
    pub items: Vec<(String, u64, u64)>,
    pub paid: bool,
}

impl Invoice {
    pub fn total_cents(&self) -> u64 {
        self.items.iter().map(|(_, price, quantity)| price * quantity).sum()
    }
}

#[derive(Default)]
pub struct Billing {
    next_number: u64,
    invoices: BTreeMap<u64, Invoice>,
    subscriptions: BTreeMap<String, (u64, u64)>,
    payments: BTreeMap<String, u64>,
}

impl Billing {
    pub fn subscribe(&mut self, account: String, monthly_cents: u64, due_at: u64) {
        self.subscriptions.insert(account, (monthly_cents, due_at));
    }

    pub fn invoice_due(&mut self, now: u64) -> Vec<Invoice> {
        let mut issued = Vec::new();
        for (account, (amount, due_at)) in &mut self.subscriptions {
            if *due_at > now { continue; }
            self.next_number += 1;
            let invoice = Invoice {
                number: self.next_number, account: account.clone(),
                items: vec![("Monthly subscription".into(), *amount, 1)], paid: false,
            };
            *due_at += 30 * 24 * 60 * 60;
            self.invoices.insert(invoice.number, invoice.clone());
            issued.push(invoice);
        }
        issued
    }

    pub fn record_payment(&mut self, receipt: String, invoice: u64, cents: u64) -> Result<(), &'static str> {
        if self.payments.contains_key(&receipt) { return Err("duplicate receipt"); }
        let bill = self.invoices.get_mut(&invoice).ok_or("unknown invoice")?;
        if bill.total_cents() != cents { return Err("amount mismatch"); }
        bill.paid = true;
        self.payments.insert(receipt, invoice);
        Ok(())
    }

    pub fn outstanding(&self, account: &str) -> Vec<&Invoice> {
        self.invoices.values().filter(|bill| bill.account == account && !bill.paid).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issues_once_and_tracks_payment() {
        let mut billing = Billing::default();
        billing.subscribe("shop".into(), 1200, 100);
        let invoice = billing.invoice_due(100).pop().unwrap();
        assert!(billing.invoice_due(101).is_empty());
        assert_eq!(billing.outstanding("shop").len(), 1);
        assert_eq!(billing.record_payment("receipt".into(), invoice.number, 1200), Ok(()));
        assert!(billing.outstanding("shop").is_empty());
    }
}
"#
}

pub fn renderer() -> &'static str {
    r#"use std::collections::BTreeMap;

pub trait ReportPlugin: Send + Sync {
    fn section(&self) -> &'static str;
    fn render(&self, rows: &[BTreeMap<String, String>]) -> String;
}

#[derive(Default)]
pub struct ReportRenderer {
    plugins: Vec<Box<dyn ReportPlugin>>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl ReportRenderer {
    pub fn register(&mut self, plugin: Box<dyn ReportPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn render(&self, rows: &[BTreeMap<String, String>]) -> String {
        let mut html = String::from("<!doctype html><html><body>");
        for plugin in &self.plugins {
            html.push_str(&format!("<section><h2>{}</h2>{}</section>",
                escape(plugin.section()), plugin.render(rows)));
        }
        html.push_str("</body></html>");
        html
    }
}

pub struct TablePlugin;
impl ReportPlugin for TablePlugin {
    fn section(&self) -> &'static str { "Table" }
    fn render(&self, rows: &[BTreeMap<String, String>]) -> String {
        rows.iter().map(|row| format!("<tr>{}</tr>", row.values()
            .map(|value| format!("<td>{}</td>", escape(value))).collect::<String>())).collect()
    }
}
"#
}

pub fn portal() -> &'static str {
    r##"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Developer documentation</title></head>
<body>
<nav><a href="#setup">Setup</a> <a href="#api">API</a> <a href="#deploy">Deployment</a></nav>
<label>Search documentation <input id="query" type="search"></label>
<main id="pages"></main>
<script>
const sections = [
  {id: 'setup', title: 'Development setup', text: 'Install Node. Run npm install and npm test. Set SERVICE_PORT to select the listener port.'},
  {id: 'api', title: 'API reference', text: 'GET /status returns service state. POST /reports creates a report. GET /reports/:id returns its rendered content.'},
  {id: 'deploy', title: 'Deployment', text: 'Build with npm run build. Copy dist to the static host. Configure cache headers and a fallback route. Verify /status before switching traffic.'},
  {id: 'operations', title: 'Operations', text: 'Retain the last stable release. On failed health checks, restore it and review the deployment log.'}
];
function render(query) {
  const main = document.getElementById('pages');
  main.replaceChildren();
  for (const page of sections.filter(p => (p.title + ' ' + p.text).toLowerCase().includes(query.toLowerCase()))) {
    const section = document.createElement('section');
    section.id = page.id;
    const heading = document.createElement('h2');
    heading.textContent = page.title;
    const text = document.createElement('p');
    text.textContent = page.text;
    section.append(heading, text);
    main.append(section);
  }
}
document.getElementById('query').addEventListener('input', event => render(event.target.value));
render('');
</script>
</body></html>
"##
}

pub fn content(scenario: &str) -> &'static str {
    match scenario {
        "optional_refactor" => renderer(),
        "optional_docs" => portal(),
        "necessary_dependency" => {
            "pub fn token_is_valid(expires_at: u64, now: u64) -> bool {\n    expires_at > now\n}\n\npub fn authenticate(expires_at: u64, now: u64) -> bool {\n    token_is_valid(expires_at, now)\n}\n\n#[test]\nfn rejects_expiry_boundary() {\n    assert!(!authenticate(100, 100));\n    assert!(authenticate(101, 100));\n}\n"
        }
        "minor_cleanup" => "pub fn token_subject(subject: &str) -> &str {\n    subject\n}\n",
        _ => billing(),
    }
}
