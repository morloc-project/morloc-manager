You are a sysadmin installing morloc system-wide for multiple users on a shared server.

Install system-wide:
- `sudo bash /vagrant/morloc-manager.sh install --system edge`
- Check info: `bash /vagrant/morloc-manager.sh info`
- Check system-specific info: `bash /vagrant/morloc-manager.sh info --system`
- Run morloc: `bash /vagrant/morloc-manager.sh run morloc --version`

After system install, verify that a regular user can use morloc:
- First, testuser must select the system version: `sudo -u testuser bash -c 'bash /vagrant/morloc-manager.sh select --system edge'`
- As testuser: `sudo -u testuser bash -c 'bash /vagrant/morloc-manager.sh run morloc --version'`
- Check that testuser sees the system install: `sudo -u testuser bash -c 'bash /vagrant/morloc-manager.sh info'`

Test that system config is in the right place:
- Config should be under /etc/morloc
- Data should be under /usr/local/share/morloc
- `ls -la /etc/morloc/` and `ls -la /usr/local/share/morloc/` to verify

Try both container engines with --system:
- `sudo bash /vagrant/morloc-manager.sh --container-engine docker install --system edge`
- `sudo bash /vagrant/morloc-manager.sh --container-engine podman install --system edge`

Test uninstall:
- `sudo bash /vagrant/morloc-manager.sh uninstall --system --all`
- Verify cleanup: config and data dirs should be removed
