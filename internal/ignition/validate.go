/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package ignition

import (
	"encoding/json"
	"fmt"

	"github.com/coreos/go-semver/semver"
	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	ignvalidate "github.com/coreos/ignition/v2/config/validate"
)

// ValidateConfig validates an ignition config struct for structural issues such as
// duplicate file paths, duplicate systemd units, path conflicts with systemd, and
// unused/unknown keys.
//
// The underlying coreos/ignition validator uses a version-specific type hierarchy
// where each version package's Validate() method enforces a strict version == MaxVersion
// equality check. Since we import v3_4/types, MaxVersion is 3.4.0, but the HCP ignition
// may legitimately arrive with an older spec version (3.2.0, 3.3.0, etc.) — all of which
// are forward-compatible with the v3_4 Go structs. The ignition-validate CLI handles this
// via ParseCompatibleVersion, which translates older configs up to the latest version
// before validation. We achieve the same effect here by normalizing the version on a
// shallow copy before running structural validation.
//
// The caller's config is not modified.
func ValidateConfig(cfg *igntypes.Config) error {
	if cfg == nil {
		return fmt.Errorf("ignition config is nil")
	}

	// Sanity-check: the version must be a valid semver with major version 3.
	// This catches truly unsupported versions (2.x, garbage strings) before
	// we normalize and validate.
	sv, err := semver.NewVersion(cfg.Ignition.Version)
	if err != nil {
		return fmt.Errorf("ignition config has invalid version %q: %w", cfg.Ignition.Version, err)
	}
	if sv.Major != 3 {
		return fmt.Errorf("ignition config has unsupported version %q: only spec 3.x is supported", cfg.Ignition.Version)
	}

	// Shallow copy so we don't mutate the caller's version string.
	// We normalize to MaxVersion (3.4.0) because the v3_4 package's validator
	// does a strict equality check, but the structural validation logic is
	// version-agnostic — it just walks Go struct fields.
	cfgCopy := *cfg
	cfgCopy.Ignition.Version = igntypes.MaxVersion.String()

	raw, err := json.Marshal(cfgCopy)
	if err != nil {
		return fmt.Errorf("failed to marshal ignition config for validation: %w", err)
	}

	rpt := ignvalidate.ValidateWithContext(cfgCopy, raw)
	if rpt.IsFatal() {
		return fmt.Errorf("ignition config validation failed: %s", rpt.String())
	}
	return nil
}
