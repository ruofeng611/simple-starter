# 新增宏rest_controller

### 使用示例如下

```rust
use crate::dto::student_dto::StudentDto;
use crate::service::student_service::StudentService;
use simple_starter_macro::{component, post_mapping, rest_controller};
use simple_starter_web::axum::extract;
use simple_starter_web::{json_response_wrap, JsonResponse};
use std::sync::Arc;

#[component]
pub struct TestController {
    #[inject]
    student_service: Arc<StudentService>,
}

#[rest_controller]
impl TestController {
    #[post_mapping("/student/add/{id}")]
    pub async fn get_student_name(
        &self,
        extract::Path(id): extract::Path<i64>,
        extract::Json(student): extract::Json<StudentDto>,
    ) -> JsonResponse {
        json_response_wrap!(function_name = "根据学生id获取学生姓名", {
            println!("id: {}", id);
            println!("student: {:?}", student);
            println!("find_student_name: {:?}", self.student_service.get_student_name(id).await);
            Ok(())
        })
    }
}

```