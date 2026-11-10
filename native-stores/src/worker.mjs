// Deliver persisted changes independently of reconciliation or individual damaged credentials.
export async function maintenanceTick(service, repository, deliver, warn = () => {}) {
  let failed = false;
  try {
    for (const value of await repository.pendingDelivery()) {
      try {
        if (await deliver(value)) await repository.delivered(value);
        else failed = true;
      } catch { failed = true; }
    }
  } catch { failed = true; }
  try { await service.maintenance(); } catch { failed = true; }
  if (failed) warn();
}
