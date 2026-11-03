{{- define "luu.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "luu.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "luu.selector" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* What a browser types: `origin`, else the ingress's host. */}}
{{- define "luu.origin" -}}
{{- if .Values.origin -}}
{{- .Values.origin -}}
{{- else if and .Values.ingress.enabled .Values.ingress.host -}}
{{- printf "%s://%s" (ternary "https" "http" .Values.ingress.tls.enabled) .Values.ingress.host -}}
{{- end -}}
{{- end -}}


{{/* The claim that holds /work. */}}
{{- define "luu.workClaim" -}}
{{- .Values.persistence.work.existingClaim | default (printf "%s-work" (include "luu.fullname" .)) -}}
{{- end -}}

{{/* Whether sessions run in pods of their own. */}}
{{- define "luu.pods" -}}
{{- if eq .Values.runtime "kubernetes" }}true{{ end -}}
{{- end -}}

{{/*
What a worker pod is labelled with, as `a=b,c=d`: never the Deployment's
selector, or its ReplicaSet would adopt the pod, and the Service would send
it traffic.
*/}}
{{- define "luu.workerLabels" -}}
app.kubernetes.io/name={{ .Chart.Name }}-worker,app.kubernetes.io/instance={{ .Release.Name }}
{{- end -}}
