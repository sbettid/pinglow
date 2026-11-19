{{- define "pinglow.timescaledb.fullname" -}}
{{- printf "%s-timescaledb" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "pinglow.timescaledb.secretName" -}}
{{- default "pinglow-db-credentials" .Values.timescaledb.secretName -}}
{{- end -}}

{{- define "pinglow.redis.secretName" -}}
{{- default "pinglow-redis-password" .Values.redis.secretName -}}
{{- end -}}

{{- define "pinglow.runner.namespace" -}}
{{- $namespace := default (printf "%s-runner" .Release.Name | trunc 63 | trimSuffix "-") .Values.runner.namespace -}}
{{- if eq $namespace .Release.Namespace -}}
{{- fail "runner.namespace must differ from the release namespace to isolate check execution" -}}
{{- end -}}
{{- $namespace -}}
{{- end -}}

{{- define "pinglow.runner.serviceAccountName" -}}
{{- printf "%s-runner" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "pinglow.runner.redis.secretName" -}}
{{- default (printf "%s-runner-redis-password" .Release.Name | trunc 63 | trimSuffix "-") .Values.runner.redis.secretName -}}
{{- end -}}

{{- define "pinglow.db.secretName" -}}
{{- default "pinglow-db-credentials" .Values.pinglow.db.secretName -}}
{{- end -}}