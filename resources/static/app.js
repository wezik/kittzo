const TAX_FORMS = { "VAT-7": "VAT-7", PPL: "PPE" };

function escapeHtml(text) {
  const div = document.createElement("div");
  div.textContent = text ?? "";
  return div.innerHTML;
}

function formatDateTime(iso) {
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatDate(isoDate) {
  const [year, month, day] = isoDate.split("-").map(Number);
  return new Date(year, month - 1, day).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

// ponytail: assumes a 2-decimal currency (fine for USD/EUR/PLN, wrong for JPY) - the
// currency's real exponent lives server-side in `Money`, not exposed over the API.
function decimals(currency) {
  return currency === "JPY" ? 0 : 2;
}

function formatMoney({ minor_amount, currency }) {
  const places = decimals(currency);
  const amount = (minor_amount / 10 ** places).toLocaleString(undefined, {
    minimumFractionDigits: places,
    maximumFractionDigits: places,
  });
  return `${amount} ${currency}`;
}

function toMinorAmount(amount, currency) {
  return Math.round(Number(amount) * 10 ** decimals(currency));
}

function transferTitle(transfer) {
  if (transfer.type === "tax") return TAX_FORMS[transfer.details.form_symbol];
  return transfer.details.title;
}

function transferSubtitle(transfer) {
  if (transfer.type === "tax") return "tax";
  return transfer.details.recipient;
}

function gmailLink(messageId) {
  const id = messageId.replace(/^<|>$/g, "");
  return `https://mail.google.com/mail/u/0/#search/rfc822msgid%3A${encodeURIComponent(id)}`;
}

function sourceLabel(source) {
  if (source.type === "SCHEDULE") return `schedule, ${formatDate(source.occurrence_date)}`;
  if (source.type === "EMAIL") {
    return `<a href="${escapeHtml(gmailLink(source.message_id))}" target="_blank" rel="noopener noreferrer">email</a>`;
  }
  return "manual";
}

function titleCell(transfer) {
  return `<div class="cell-title">${escapeHtml(transferTitle(transfer))}</div>
    <div class="cell-sub">${escapeHtml(transferSubtitle(transfer))}</div>`;
}

async function api(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { "Content-Type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    alert((await res.text()) || `${method} ${url} failed with ${res.status}`);
    return null;
  }
  return res.status === 204 ? true : res.json();
}

function render(id, rows, emptyId) {
  document.getElementById(emptyId).hidden = rows.length > 0;
  document.getElementById(id).innerHTML = rows.join("");
}

// Payments

const expanded = new Set();
let lastPayments = [];

function formatAccountNumber(number) {
  return `${number.slice(0, 2)} ${number.slice(2).match(/.{1,4}/g).join(" ")}`;
}

function statusCell(p) {
  if (p.status === "paid") return `<span class="ok">paid</span>`;
  if (p.ack === "pending") return `<span class="pending">awaiting approval</span>`;
  if (p.ack === "rejected") return `<span class="muted">rejected</span>`;
  return `unpaid <button class="primary" data-pay="${p.id}">Mark paid</button>`;
}

function recipientCell(p) {
  if (p.type === "tax") return `<span class="muted">tax office</span>`;
  return escapeHtml(p.details.recipient);
}

function paymentDetails(p) {
  const rows = [
    p.type === "standard" && ["Account number", escapeHtml(formatAccountNumber(p.details.account_number))],
    ["Source", sourceLabel(p.source)],
    p.paid_at && ["Paid", formatDateTime(p.paid_at)],
    ["Created", formatDateTime(p.created_at)],
    ["Updated", formatDateTime(p.updated_at)],
  ].filter(Boolean);
  return `<dl class="details">${rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>`;
}

function renderPayments(payments) {
  lastPayments = payments;
  const newestFirst = [...payments].reverse();

  render(
    "confirmations-body",
    newestFirst
      .filter((p) => p.ack === "pending")
      .map(
        (p) => `<div class="confirmation-row expandable${expanded.has(p.id) ? " open" : ""}" data-toggle="${p.id}">
          <div>
            <div class="cell-title">${escapeHtml(transferTitle(p))} &middot; ${formatMoney(p.total)}</div>
            <div class="cell-sub">${recipientCell(p)}</div>
          </div>
          <div class="actions">
            <button class="primary" data-ack="${p.id}" data-accept="true">Approve</button>
            <button class="danger" data-ack="${p.id}" data-accept="false">Reject</button>
          </div>
          ${expanded.has(p.id) ? paymentDetails(p) : ""}
        </div>`,
      ),
    "confirmations-empty",
  );

  render(
    "payments-body",
    newestFirst.map(
      (p) => `<tr class="expandable${expanded.has(p.id) ? " open" : ""}" data-toggle="${p.id}">
        <td class="cell-title">${escapeHtml(transferTitle(p))}</td>
        <td class="wrap">${recipientCell(p)}</td>
        <td class="amount">${formatMoney(p.total)}</td>
        <td>${statusCell(p)}</td>
      </tr>${
        expanded.has(p.id) ? `<tr class="detail-row"><td colspan="4">${paymentDetails(p)}</td></tr>` : ""
      }`,
    ),
    "payments-empty",
  );
}

function toggleDetails(e) {
  if (e.target.closest("button, a, .details")) return false;
  const row = e.target.closest("[data-toggle]");
  if (!row) return false;
  const id = row.dataset.toggle;
  expanded.has(id) ? expanded.delete(id) : expanded.add(id);
  renderPayments(lastPayments);
  return true;
}

document.getElementById("confirmations-body").addEventListener("click", async (e) => {
  if (toggleDetails(e)) return;
  const button = e.target.closest("button[data-ack]");
  if (!button) return;
  await api("POST", `/payments/${button.dataset.ack}/ack`, {
    accept: button.dataset.accept === "true",
  });
  refresh();
});

document.getElementById("payments-body").addEventListener("click", async (e) => {
  if (toggleDetails(e)) return;
  const button = e.target.closest("button[data-pay]");
  if (!button) return;
  await api("POST", `/payments/${button.dataset.pay}/pay`);
  refresh();
});

// Schedules

let schedulesById = {};

function renderSchedules(schedules) {
  schedulesById = Object.fromEntries(schedules.map((s) => [s.id, s]));
  render(
    "schedules-body",
    schedules.map(
      (s) => `<tr>
        <td>${titleCell(s)}</td>
        <td class="amount">${formatMoney(s.total)}</td>
        <td>day ${s.recurrence.day_of_month}</td>
        <td>${formatDate(s.next_due_at)}</td>
        <td class="actions">
          <button class="danger" data-delete="${s.id}">Delete</button>
        </td>
      </tr>`,
    ),
    "schedules-empty",
  );
}

document.getElementById("schedules-body").addEventListener("click", async (e) => {
  const remove = e.target.closest("button[data-delete]");
  if (!remove) return;
  const title = transferTitle(schedulesById[remove.dataset.delete]);
  if (!confirm(`Delete schedule "${title}"? Payments it already created stay.`)) return;
  await api("DELETE", `/payment-schedules/${remove.dataset.delete}`);
  refresh();
});

// Forms

function showKindFields(form) {
  const type = form.elements.type.value;
  form.querySelectorAll("[data-kind]").forEach((label) => {
    const active = label.dataset.kind === type;
    label.hidden = !active;
    label.querySelectorAll("input, select").forEach((field) => (field.disabled = !active));
  });
}

function transferBody(form) {
  const currency = form.get("currency");
  const type = form.get("type");
  const details =
    type === "tax"
      ? { form_symbol: form.get("form_symbol") }
      : {
          title: form.get("title"),
          account_number: form.get("account_number"),
          recipient: form.get("recipient"),
        };
  return {
    total: { minor_amount: toMinorAmount(form.get("amount"), currency), currency },
    type,
    details,
  };
}

{
  const form = document.getElementById("payment-form");
  form.elements.type.addEventListener("change", () => showKindFields(form));
  form.addEventListener("reset", () => setTimeout(() => showKindFields(form)));
  showKindFields(form);
}

document.getElementById("payment-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (await api("POST", "/payments", transferBody(new FormData(e.target)))) e.target.reset();
  refresh();
});

document.getElementById("schedule-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  const currency = form.get("currency");
  const body = {
    total: { minor_amount: toMinorAmount(form.get("amount"), currency), currency },
    title_template: form.get("title_template"),
    account_number: form.get("account_number"),
    recipient: form.get("recipient"),
    recurrence: { type: "monthly", day_of_month: Number(form.get("day_of_month")) },
    starts_at: form.get("starts_at") || undefined,
  };
  if (await api("POST", "/payment-schedules", body)) e.target.reset();
  refresh();
});

// Refresh

async function refresh() {
  const [payments, schedules] = await Promise.all([
    fetch("/payments").then((r) => r.json()),
    fetch("/payment-schedules").then((r) => r.json()),
  ]);
  renderPayments(payments);
  renderSchedules(schedules);
}

refresh();
setInterval(refresh, 2000);
