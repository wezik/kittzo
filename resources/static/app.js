async function loadConfirmations() {
  const confirmations = await fetch("/confirmations?state=pending").then((r) => r.json());
  document.getElementById("confirmations-body").innerHTML = confirmations
    .map((c) => {
      const subject = c.subject.type === "payment" ? `payment ${c.subject.payment_id}` : c.subject.type;
      return `<tr>
        <td>${subject}</td>
        <td>${c.created_at}</td>
        <td>
          <button data-id="${c.id}" data-approved="true">Approve</button>
          <button data-id="${c.id}" data-approved="false">Reject</button>
        </td>
      </tr>`;
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
  document.getElementById("payments-body").innerHTML = payments
    .map((p) => {
      const source = p.source.type === "manual" ? "manual" : `schedule (${p.source.occurrence_date})`;
      return `<tr><td>${p.total.amount} ${p.total.currency}</td><td>${source}</td><td>${p.confirmation_state}</td><td>${p.created_at}</td></tr>`;
    })
    .join("");
}

async function loadSchedules() {
  const schedules = await fetch("/payment-schedules").then((r) => r.json());
  document.getElementById("schedules-body").innerHTML = schedules
    .map((s) => {
      const r = s.recurrence;
      const recurrence = `every ${r.interval_months}mo on day ${r.day_of_month}`;
      return `<tr><td>${s.total.amount} ${s.total.currency}</td><td>${recurrence}</td><td>${s.status}</td><td>${s.last_run_at ?? "—"}</td></tr>`;
    })
    .join("");
}

document.getElementById("payment-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  await fetch("/ingest", {
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
        type: "every_n_months",
        interval_months: Number(form.get("interval_months")),
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
