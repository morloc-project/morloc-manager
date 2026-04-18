You are a sysadmin installing morloc system-wide for multiple users on a shared server.

Create a system environment:
- `sudo /vagrant/morloc-manager new shared --version 0.76.0 --system`
- Check info: `/vagrant/morloc-manager info`

After system install, verify that a regular user can use morloc:
- First, testuser must select the system environment:
  `sudo -u testuser /vagrant/morloc-manager select shared`
- As testuser:
  `sudo -u testuser bash -c 'cd /tmp/testdir && /vagrant/morloc-manager run -- morloc --version'`
- Check that testuser sees the system environment:
  `sudo -u testuser /vagrant/morloc-manager info`

Test that system config is in the right place:
- Config should be under /etc/morloc/environments/shared/
- Data should be under /usr/local/share/morloc/environments/shared/
- `ls -la /etc/morloc/environments/` and `ls -la /usr/local/share/morloc/environments/`

Try system environments with both engines:
- `sudo /vagrant/morloc-manager new sys-docker --version 0.76.0 --system --engine docker`
- `sudo /vagrant/morloc-manager new sys-podman --version 0.76.0 --system --engine podman`

After system install, verify testuser can create their own local environment:
- `sudo -u testuser /vagrant/morloc-manager new mylocal --version 0.76.0`

Test that the system environment with a Dockerfile layer works:
- `sudo /vagrant/morloc-manager new custom --version 0.76.0 --system --dockerfile-stub`
- Edit /etc/morloc/environments/custom/Dockerfile
- `sudo /vagrant/morloc-manager update custom`
- Verify a regular user can use the custom environment
