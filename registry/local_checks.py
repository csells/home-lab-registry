"""Site-specific checks belong in private deployment configuration."""
CHECKS = set()
def check(name):
    return False, 'No site-specific check installed'
