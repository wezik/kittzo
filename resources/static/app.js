function formatDateTime(iso) {
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function shortId(id) {
  return id.slice(0, 8);
}

async function loadConfirmations() {
  const confirmations = await fetch("/confirmations?state=pending").then((r) => r.json());
  document.getElementById("confirmations-empty").hidden = confirmations.length > 0;
  document.getElementById("confirmations-body").innerHTML = confirmations
    .map((c) => {
      const subject =
        c.subject.type === "payment"
          ? `Payment <span title="${c.subject.payment_id}">${shortId(c.subject.payment_id)}</span>`
          : c.subject.type;
      return `<div class="confirmation-row">
        <div class="confirmation-meta">
          <div class="confirmation-subject">${subject}</div>
          <div class="confirmation-time">${formatDateTime(c.created_at)}</div>
        </div>
        <div class="confirmation-actions">
          <button data-id="${c.id}" data-approved="true">Approve</button>
          <button data-id="${c.id}" data-approved="false">Reject</button>
        </div>
      </div>`;
    })
    .join("");
}

document.getElementById("confirmations-body").addEventListener("click", async (e) => {
  const button = e.target.closest("button[data-id]");
  if (!button) return;
  await fetch(`/confirmations/${button.dataset.id}/decide`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ approved: button.dataset.approved === "true" }),
  });
  loadConfirmations();
  loadPayments();
});

async function loadPayments() {
  const payments = await fetch("/payments").then((r) => r.json());
  document.getElementById("payments-empty").hidden = payments.length > 0;
  document.getElementById("payments-body").innerHTML = payments
    .map((p) => {
      const source = p.source.type === "manual" ? "manual" : `schedule (${p.source.occurrence_date})`;
      return `<tr>
        <td class="amount">${p.total.amount} ${p.total.currency}</td>
        <td>${source}</td>
        <td><span class="state-${p.confirmation_state}">${p.confirmation_state}</span></td>
        <td>${formatDateTime(p.created_at)}</td>
      </tr>`;
    })
    .join("");
}

async function loadSchedules() {
  const schedules = await fetch("/payment-schedules").then((r) => r.json());
  document.getElementById("schedules-empty").hidden = schedules.length > 0;
  document.getElementById("schedules-body").innerHTML = schedules
    .map((s) => {
      const r = s.recurrence;
      const recurrence = `monthly on day ${r.day_of_month}`;
      return `<tr>
        <td class="amount">${s.total.amount} ${s.total.currency}</td>
        <td>${recurrence}</td>
        <td>${s.status}</td>
        <td>${formatDateTime(s.next_due_at)}</td>
      </tr>`;
    })
    .join("");
}

document.getElementById("payment-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  await fetch("/payments", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      total: { amount: form.get("amount"), currency: form.get("currency") },
    }),
  });
  e.target.reset();
  loadPayments();
  loadConfirmations();
});

document.getElementById("schedule-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  await fetch("/payment-schedules", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      total: { amount: form.get("amount"), currency: form.get("currency") },
      recurrence: {
        type: "monthly",
        day_of_month: Number(form.get("day_of_month")),
      },
    }),
  });
  e.target.reset();
  loadSchedules();
});

loadConfirmations();
loadPayments();
loadSchedules();

setInterval(() => {
  loadConfirmations();
  loadPayments();
  loadSchedules();
}, 2000);
