{{- define "pinglow.timescaledb.fullname" -}}
{{- printf "%s-timescaledb" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "pinglow.timescaledb.secretName" -}}
{{- default (printf "%s-credentials" (include "pinglow.timescaledb.fullname" .)) .Values.timescaledb.existingSecret -}}
{{- end -}}