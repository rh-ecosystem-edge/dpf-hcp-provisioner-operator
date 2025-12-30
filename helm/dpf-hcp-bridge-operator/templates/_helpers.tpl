{{/*
Expand the name of the chart.
*/}}
{{- define "dpf-hcp-bridge-operator.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
Simplified to use just release name to avoid long concatenated names.
*/}}
{{- define "dpf-hcp-bridge-operator.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "dpf-hcp-bridge-operator.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "dpf-hcp-bridge-operator.labels" -}}
helm.sh/chart: {{ include "dpf-hcp-bridge-operator.chart" . }}
{{ include "dpf-hcp-bridge-operator.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: dpf-hcp-bridge-operator
{{- with .Values.commonLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "dpf-hcp-bridge-operator.selectorLabels" -}}
app.kubernetes.io/name: {{ include "dpf-hcp-bridge-operator.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
control-plane: controller-manager
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "dpf-hcp-bridge-operator.serviceAccountName" -}}
{{- default .Values.serviceAccount.name (include "dpf-hcp-bridge-operator.fullname" .) }}
{{- end }}

{{/*
Namespace
*/}}
{{- define "dpf-hcp-bridge-operator.namespace" -}}
{{- .Values.namespace | default "dpf-hcp-bridge-system" }}
{{- end }}

{{/*
Image
*/}}
{{- define "dpf-hcp-bridge-operator.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion }}
{{- printf "%s:%s" .Values.image.repository $tag }}
{{- end }}
