You are a sysadmin installing morloc system-wide for multiple users on a shared server.

Use sudo and --system flag for everything:
- Install system-wide: `sudo bash /vagrant/morloc-manager.sh --system install edge`
- Check info: `sudo bash /vagrant/morloc-manager.sh --system info`
- Run as root: `sudo bash /vagrant/morloc-manager.sh --system run morloc --version`

After system install, verify that a regular user can use morloc:
- As testuser: `sudo -u testuser bash -c 'bash /vagrant/morloc-manager.sh run morloc --version'`
- Check that testuser sees the system install: `sudo -u testuser bash -c 'bash /vagrant/morloc-manager.sh info'`

Test that system config is in the right place:
- Config should be under /etc/morloc
- Data should be under /usr/local/share/morloc
- `ls -la /etc/morloc/` and `ls -la /usr/local/share/morloc/` to verify

Try both container engines with --system:
- `sudo bash /vagrant/morloc-manager.sh --system --container-engine docker install edge`
- `sudo bash /vagrant/morloc-manager.sh --system --container-engine podman install edge`

Test uninstall:
- `sudo bash /vagrant/morloc-manager.sh --system uninstall edge`
- Verify cleanup: config and data dirs should be removed
