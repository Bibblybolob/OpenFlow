export function updateInstallStatus(installed: boolean): string {
  return installed ? "Installed. Restarting…" : "No update is available.";
}
